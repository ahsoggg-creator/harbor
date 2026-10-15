//! Finds programs of the listed repositories that are already installed on this PC: first through the Windows
//! "Apps & features" registry entries, then by looking for a folder or an exe named like the repository in the usual
//! places. Nothing is changed or executed; the scan only reads.

use crate::model::{Detected, Repo};
use crate::winint;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub struct RegEntry {
    pub key_name: String,
    pub display_name: String,
    pub version: Option<String>,
    pub location: Option<String>,
    pub icon: Option<String>,
    pub uninstall: Option<String>,
    pub urls: Vec<String>,
}

fn norm(s: &str) -> String {
    s.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

const READ_REGISTRY: &str = "[Console]::OutputEncoding = [Text.Encoding]::UTF8
$k = 'HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\*', 'HKLM:\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\*', 'HKLM:\\Software\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\*'
Get-ItemProperty $k -ErrorAction SilentlyContinue | Where-Object { $_.DisplayName } | Select-Object PSChildName, DisplayName, DisplayVersion, InstallLocation, DisplayIcon, UninstallString, URLInfoAbout, HelpLink, URLUpdateInfo | ConvertTo-Json -Compress";

pub fn read_registry() -> Vec<RegEntry> {
    let Ok(out) = winint::powershell_output(READ_REGISTRY) else {
        return Vec::new();
    };
    parse_registry(&out)
}

fn parse_registry(json: &str) -> Vec<RegEntry> {
    let Ok(v) = serde_json::from_str::<Value>(json.trim()) else {
        return Vec::new();
    };
    // one result is printed as an object, several as an array
    let items = match v {
        Value::Array(a) => a,
        o @ Value::Object(_) => vec![o],
        _ => Vec::new(),
    };
    let get = |v: &Value, k: &str| {
        v.get(k)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    items
        .iter()
        .map(|v| RegEntry {
            key_name: get(v, "PSChildName").unwrap_or_default(),
            display_name: get(v, "DisplayName").unwrap_or_default(),
            version: get(v, "DisplayVersion"),
            location: get(v, "InstallLocation"),
            icon: get(v, "DisplayIcon"),
            uninstall: get(v, "UninstallString"),
            urls: ["URLInfoAbout", "HelpLink", "URLUpdateInfo"]
                .iter()
                .filter_map(|k| get(v, k))
                .collect(),
        })
        .collect()
}

fn url_points_to(url: &str, repo: &Repo) -> bool {
    let needle = format!("github.com/{}/{}", repo.owner, repo.name).to_lowercase();
    let url = url.to_lowercase();
    url.match_indices(&needle).any(|(i, _)| {
        url[i + needle.len()..]
            .chars()
            .next()
            .is_none_or(|c| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')))
    })
}

/// 2 = the entry points at the repository URL, 1 = same name (optionally followed by a version), 0 = no match.
fn match_score(e: &RegEntry, repo: &Repo) -> u8 {
    if e.urls.iter().any(|u| url_points_to(u, repo)) {
        return 2;
    }
    let name = norm(&repo.name);
    if name.is_empty() {
        return 0;
    }
    let (display, child) = (norm(&e.display_name), norm(&e.key_name));
    let versioned = display
        .strip_prefix(&name)
        .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit() || c == 'v' || c == 'x'));
    u8::from(display == name || child == name || versioned)
}

fn find_exe_in(dir: &Path, name: &str) -> Option<PathBuf> {
    let name = norm(name);
    let exes: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("exe")))
        .collect();
    let stem = |p: &PathBuf| {
        norm(
            &p.file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
        )
    };
    exes.iter()
        .find(|p| stem(p) == name)
        .or_else(|| {
            exes.iter()
                .find(|p| !stem(p).contains("unins") && !stem(p).contains("setup"))
        })
        .cloned()
}

fn resolve_path(e: &RegEntry, repo: &Repo) -> Option<PathBuf> {
    if let Some(icon) = &e.icon {
        let icon = icon.trim().trim_matches('"');
        let icon = match icon.rfind(',') {
            Some(i) if icon[i + 1..].trim().parse::<i32>().is_ok() => &icon[..i],
            _ => icon,
        };
        let p = PathBuf::from(icon.trim().trim_matches('"'));
        if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("exe")) && p.exists() {
            return Some(p);
        }
    }
    let dir = PathBuf::from(e.location.as_deref()?.trim_matches('"'));
    dir.is_dir().then(|| find_exe_in(&dir, &repo.name).unwrap_or(dir))
}

pub fn match_registry(entries: &[RegEntry], repos: &[Repo]) -> BTreeMap<String, Detected> {
    let mut found = BTreeMap::new();
    for repo in repos {
        let best = entries
            .iter()
            .filter_map(|e| {
                let score = match_score(e, repo);
                (score > 0).then(|| resolve_path(e, repo).map(|p| (score, e, p)))?
            })
            .max_by_key(|(score, ..)| *score);
        if let Some((_, e, path)) = best {
            found.insert(
                repo.key(),
                Detected {
                    path,
                    version: e.version.clone(),
                    source: "Приложения Windows",
                    uninstall: e.uninstall.clone(),
                },
            );
        }
    }
    found
}

/// `app_roots` hold one folder per program (`Programs\Name\Name.exe`); `loose_roots` may hold a bare `Name.exe`.
pub fn scan_folders(
    app_roots: &[PathBuf],
    loose_roots: &[PathBuf],
    repos: &[Repo],
    found: &mut BTreeMap<String, Detected>,
) {
    for repo in repos {
        let key = repo.key();
        if found.contains_key(&key) {
            continue;
        }
        let name = norm(&repo.name);
        let mut hit = None;
        for root in app_roots {
            let dir = std::fs::read_dir(root)
                .into_iter()
                .flatten()
                .flatten()
                .find(|e| e.path().is_dir() && norm(&e.file_name().to_string_lossy()) == name);
            if let Some(d) = dir {
                let p = d.path();
                hit = Some((find_exe_in(&p, &repo.name).unwrap_or(p), "папка программы"));
                break;
            }
        }
        if hit.is_none() {
            for root in loose_roots {
                let file = std::fs::read_dir(root).into_iter().flatten().flatten().find(|e| {
                    let p = e.path();
                    p.extension().is_some_and(|x| x.eq_ignore_ascii_case("exe"))
                        && norm(
                            &p.file_stem()
                                .map(|s| s.to_string_lossy().into_owned())
                                .unwrap_or_default(),
                        ) == name
                });
                if let Some(f) = file {
                    hit = Some((f.path(), "файл на ПК"));
                    break;
                }
            }
        }
        if let Some((path, source)) = hit {
            found.insert(
                key,
                Detected {
                    path,
                    version: None,
                    source,
                    uninstall: None,
                },
            );
        }
    }
}

fn env_path(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).map(PathBuf::from)
}

pub fn scan(repos: &[Repo], harbor_downloads: &Path) -> BTreeMap<String, Detected> {
    let mut found = match_registry(&read_registry(), repos);
    let local = env_path("LOCALAPPDATA");
    let home = env_path("USERPROFILE");
    let app_roots: Vec<PathBuf> = [
        local.as_ref().map(|l| l.join("Programs")),
        env_path("ProgramFiles"),
        env_path("ProgramFiles(x86)"),
        local,
    ]
    .into_iter()
    .flatten()
    .collect();
    let loose_roots: Vec<PathBuf> = [
        home.as_ref().map(|h| h.join("Downloads")),
        home.as_ref().map(|h| h.join("Desktop")),
    ]
    .into_iter()
    .flatten()
    .filter(|p| p != harbor_downloads)
    .collect();
    scan_folders(&app_roots, &loose_roots, repos, &mut found);
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(name: &str) -> Repo {
        Repo {
            owner: "me".into(),
            name: name.into(),
            description: String::new(),
            language: None,
            stars: 0,
            private: false,
            archived: false,
            fork: false,
            pushed_at: String::new(),
            html_url: String::new(),
            default_branch: "main".into(),
            release: None,
            manual: false,
        }
    }

    fn entry(display: &str, dir: &Path) -> RegEntry {
        RegEntry {
            key_name: display.into(),
            display_name: display.into(),
            version: Some("1.2.3".into()),
            location: Some(dir.display().to_string()),
            icon: None,
            uninstall: Some("\"x.exe\" --uninstall".into()),
            urls: Vec::new(),
        }
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("harbor-scan-{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn registry_matches_by_name_version_and_url() {
        let dir = tmp("reg");
        std::fs::write(dir.join("SnapForge.exe"), b"x").unwrap();
        let repos = [repo("snapforge"), repo("clip"), repo("other-thing")];
        let mut by_url = entry("Totally Different", &dir);
        by_url.urls = vec!["https://github.com/me/other-thing/".into()];
        let entries = [entry("SnapForge 0.7.0", &dir), entry("ClipVault Pro", &dir), by_url];
        let found = match_registry(&entries, &repos);
        assert_eq!(found["me/snapforge"].version.as_deref(), Some("1.2.3"));
        assert!(found["me/snapforge"].path.ends_with("SnapForge.exe"));
        assert!(!found.contains_key("me/clip"), "a longer word is not a version suffix");
        assert!(found.contains_key("me/other-thing"));
        assert!(!url_points_to(
            "https://github.com/me/other-thing-2",
            &repo("other-thing")
        ));
    }

    #[test]
    fn stale_registry_entries_are_ignored() {
        let found = match_registry(&[entry("Ghost", Path::new(r"C:\no\such\dir"))], &[repo("ghost")]);
        assert!(found.is_empty());
    }

    #[test]
    fn folder_scan_finds_program_folders_and_loose_exes() {
        let apps = tmp("apps");
        let loose = tmp("loose");
        std::fs::create_dir_all(apps.join("Strela")).unwrap();
        std::fs::write(apps.join("Strela").join("unins000.exe"), b"x").unwrap();
        std::fs::write(apps.join("Strela").join("Strela.exe"), b"x").unwrap();
        std::fs::write(loose.join("Packrat.exe"), b"x").unwrap();
        std::fs::write(loose.join("Packrat-Setup.exe"), b"x").unwrap();
        let mut found = BTreeMap::new();
        scan_folders(
            &[apps],
            &[loose],
            &[repo("strela"), repo("packrat"), repo("absent")],
            &mut found,
        );
        assert!(found["me/strela"].path.ends_with("Strela.exe"));
        assert!(found["me/packrat"].path.ends_with("Packrat.exe"));
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn registry_json_with_one_or_many_items() {
        let one = r#"{"PSChildName":"a","DisplayName":"A","DisplayVersion":null,"URLInfoAbout":"u"}"#;
        assert_eq!(parse_registry(one).len(), 1);
        let many = format!("[{one},{one}]");
        assert_eq!(parse_registry(&many).len(), 2);
        assert!(parse_registry("").is_empty());
    }

    #[test]
    #[ignore = "reads the real registry"]
    fn live_registry_scan_runs() {
        let entries = read_registry();
        eprintln!("{} uninstall entries", entries.len());
        assert!(!entries.is_empty());
    }
}
