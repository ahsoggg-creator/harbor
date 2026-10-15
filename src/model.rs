use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub api_url: String,
    pub size: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Release {
    pub tag: String,
    pub name: String,
    pub published_at: String,
    pub notes: String,
    pub assets: Vec<Asset>,
}

impl Release {
    /// Assets worth offering to the user: everything except checksum / signature side files.
    pub fn downloadable(&self) -> Vec<usize> {
        self.assets
            .iter()
            .enumerate()
            .filter(|(_, a)| !is_side_file(&a.name))
            .map(|(i, _)| i)
            .collect()
    }

    /// Installer first, then a plain exe, then archives.
    pub fn best_asset(&self) -> Option<usize> {
        self.downloadable()
            .into_iter()
            .min_by_key(|&i| rank(&self.assets[i].name))
    }

    pub fn checksum_asset(&self) -> Option<&Asset> {
        self.assets.iter().find(|a| {
            let n = a.name.to_lowercase();
            n.starts_with("sha256sums") || n.ends_with(".sha256") || n == "checksums.txt"
        })
    }
}

fn is_side_file(name: &str) -> bool {
    let n = name.to_lowercase();
    n.starts_with("sha256sums")
        || n.ends_with(".sha256")
        || n.ends_with(".sig")
        || n.ends_with(".asc")
        || n.ends_with(".minisig")
        || n == "checksums.txt"
}

fn rank(name: &str) -> u8 {
    let n = name.to_lowercase();
    if n.contains("setup") || n.contains("install") || n.ends_with(".msi") {
        0
    } else if n.ends_with(".exe") {
        1
    } else if n.ends_with(".zip") || n.ends_with(".7z") {
        2
    } else {
        3
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Repo {
    pub owner: String,
    pub name: String,
    pub description: String,
    pub language: Option<String>,
    pub stars: u32,
    pub private: bool,
    pub archived: bool,
    pub fork: bool,
    pub pushed_at: String,
    pub html_url: String,
    pub default_branch: String,
    pub release: Option<Release>,
    #[serde(default)]
    pub manual: bool,
}

impl Repo {
    pub fn key(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
}

pub const SOURCE_TAG: &str = "source";

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Installed {
    pub tag: String,
    pub asset: String,
    pub path: PathBuf,
    pub size: u64,
    pub at: i64,
}

/// A program of one of the repositories that is already on this PC (found by a scan, not downloaded by Harbor).
#[derive(Clone, Debug, PartialEq)]
pub struct Detected {
    pub path: PathBuf,
    pub version: Option<String>,
    pub source: &'static str,
    pub uninstall: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Settings {
    pub username: String,
    pub token: String,
    pub use_gh_token: bool,
    pub download_dir: PathBuf,
    pub refresh_minutes: u32,
    pub include_forks: bool,
    pub include_archived: bool,
    pub include_private: bool,
    pub delete_old: bool,
    pub auto_update: bool,
    pub list_mode: bool,
}

impl Default for Settings {
    fn default() -> Self {
        let home = std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        Self {
            username: "ahsoggg-creator".into(),
            token: String::new(),
            use_gh_token: true,
            download_dir: home.join("Downloads").join("Harbor"),
            refresh_minutes: 10,
            include_forks: false,
            include_archived: true,
            include_private: true,
            delete_old: true,
            auto_update: false,
            list_mode: false,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct LogEntry {
    pub at: i64,
    pub text: String,
}

#[derive(Serialize, Deserialize, Default, Debug)]
#[serde(default)]
pub struct State {
    pub settings: Settings,
    pub repos: Vec<Repo>,
    pub hidden: BTreeSet<String>,
    pub favorites: BTreeSet<String>,
    pub installed: BTreeMap<String, Installed>,
    pub manual: BTreeSet<String>,
    pub first_seen: BTreeMap<String, i64>,
    pub log: Vec<LogEntry>,
    pub last_sync: i64,
    #[serde(skip)]
    pub detected: BTreeMap<String, Detected>,
}

impl State {
    pub fn update_available(&self, repo: &Repo) -> bool {
        let key = repo.key();
        match (self.installed.get(&key), &repo.release) {
            (Some(i), Some(r)) => i.tag != SOURCE_TAG && i.tag != r.tag,
            (None, Some(r)) => self
                .detected
                .get(&key)
                .and_then(|d| d.version.as_deref())
                .is_some_and(|v| crate::update::is_newer(&r.tag, v)),
            _ => false,
        }
    }

    pub fn push_log(&mut self, text: impl Into<String>) {
        self.log.push(LogEntry {
            at: now(),
            text: text.into(),
        });
        let excess = self.log.len().saturating_sub(300);
        self.log.drain(..excess);
    }
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn state_path() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("Harbor").join("state.json")
}

pub fn load() -> State {
    std::fs::read(state_path())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn save(state: &State) {
    let path = state_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("json.tmp");
    if let Ok(bytes) = serde_json::to_vec_pretty(state)
        && std::fs::write(&tmp, bytes).is_ok()
    {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// Folder name safe to join under the download dir (GitHub names may contain anything the API returns).
pub fn sanitize(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| {
            if matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.');
    if trimmed.is_empty() {
        "_".into()
    } else {
        trimmed.to_string()
    }
}

pub fn repo_dir(download_dir: &Path, repo_name: &str) -> PathBuf {
    download_dir.join(sanitize(repo_name))
}

pub fn human_size(bytes: u64) -> String {
    const U: [&str; 4] = ["Б", "КБ", "МБ", "ГБ"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{bytes} Б")
    } else {
        format!("{v:.1} {}", U[i])
    }
}

pub fn ago_iso(iso: &str) -> String {
    match chrono::DateTime::parse_from_rfc3339(iso) {
        Ok(t) => ago_secs(now() - t.timestamp()),
        Err(_) => "—".into(),
    }
}

pub fn ago_secs(d: i64) -> String {
    match d {
        d if d < 60 => "только что".into(),
        d if d < 3600 => format!("{} мин. назад", d / 60),
        d if d < 86_400 => format!("{} ч назад", d / 3600),
        d if d < 86_400 * 30 => format!("{} дн. назад", d / 86_400),
        d if d < 86_400 * 365 => format!("{} мес. назад", d / (86_400 * 30)),
        d => format!("{} г. назад", d / (86_400 * 365)),
    }
}

pub fn clock(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|t| t.with_timezone(&chrono::Local).format("%d.%m %H:%M:%S").to_string())
        .unwrap_or_default()
}

/// Accepts `owner/name`, `https://github.com/owner/name(.git)` and trailing path pieces.
pub fn parse_repo_ref(input: &str) -> Option<(String, String)> {
    let s = input.trim().trim_end_matches('/');
    let s = s.strip_suffix(".git").unwrap_or(s);
    let s = s.split("github.com/").last().unwrap_or(s);
    let mut it = s.split('/').filter(|p| !p.is_empty());
    let (o, n) = (it.next()?, it.next()?);
    let ok = |p: &str| {
        p.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    (ok(o) && ok(n)).then(|| (o.to_string(), n.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(names: &[&str]) -> Release {
        Release {
            tag: "v1".into(),
            name: "v1".into(),
            published_at: String::new(),
            notes: String::new(),
            assets: names
                .iter()
                .map(|n| Asset {
                    name: (*n).into(),
                    url: String::new(),
                    api_url: String::new(),
                    size: 1,
                })
                .collect(),
        }
    }

    #[test]
    fn best_asset_prefers_setup_and_skips_checksums() {
        let r = rel(&["SHA256SUMS.txt", "app-cli.exe", "App-Setup.exe", "App.exe"]);
        assert_eq!(r.assets[r.best_asset().unwrap()].name, "App-Setup.exe");
        assert_eq!(r.downloadable().len(), 3);
        assert_eq!(r.checksum_asset().unwrap().name, "SHA256SUMS.txt");
    }

    #[test]
    fn repo_ref_parsing() {
        assert_eq!(
            parse_repo_ref("https://github.com/a/b.git"),
            Some(("a".into(), "b".into()))
        );
        assert_eq!(parse_repo_ref(" a/b "), Some(("a".into(), "b".into())));
        assert_eq!(
            parse_repo_ref("https://github.com/a/b/releases"),
            Some(("a".into(), "b".into()))
        );
        assert_eq!(parse_repo_ref("justname"), None);
        assert_eq!(parse_repo_ref("a b/c"), None);
    }

    #[test]
    fn sanitize_blocks_traversal() {
        assert_eq!(sanitize("..\\evil"), "_evil");
        assert_eq!(sanitize(".."), "_");
        assert_eq!(sanitize("a/b:c"), "a_b_c");
    }

    #[test]
    fn update_flag() {
        let mut st = State::default();
        let repo = Repo {
            owner: "o".into(),
            name: "n".into(),
            description: String::new(),
            language: None,
            stars: 0,
            private: false,
            archived: false,
            fork: false,
            pushed_at: String::new(),
            html_url: String::new(),
            default_branch: "main".into(),
            release: Some(rel(&["a.exe"])),
            manual: false,
        };
        assert!(!st.update_available(&repo));
        let inst = Installed {
            tag: "v0".into(),
            asset: "a.exe".into(),
            path: PathBuf::new(),
            size: 1,
            at: 0,
        };
        st.installed.insert("o/n".into(), inst.clone());
        assert!(st.update_available(&repo));
        st.installed.insert(
            "o/n".into(),
            Installed {
                tag: SOURCE_TAG.into(),
                ..inst
            },
        );
        assert!(!st.update_available(&repo));
    }
}
