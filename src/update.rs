//! Self-update: Harbor is itself one of the listed repositories, so a newer release of it shows up in the same
//! sync. The new `Harbor.exe` is downloaded with the usual SHA-256 check and swapped in place (the running file
//! is renamed to `Harbor.old`, which the next start removes).

use crate::github::{self, Client, Progress, Target};
use crate::model::{Repo, Settings};

pub const OWNER: &str = "ahsoggg-creator";
pub const REPO: &str = "harbor";
pub const ASSET: &str = "Harbor.exe";

pub struct Available {
    pub tag: String,
}

fn parts(v: &str) -> Vec<u64> {
    v.trim_start_matches('v')
        .split('.')
        .map(|p| {
            p.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .unwrap_or(0)
        })
        .collect()
}

pub fn is_newer(tag: &str, current: &str) -> bool {
    let (a, b) = (parts(tag), parts(current));
    (0..a.len().max(b.len()))
        .map(|i| (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0)))
        .find(|(x, y)| x != y)
        .is_some_and(|(x, y)| x > y)
}

pub fn find(repos: &[Repo]) -> Option<Available> {
    let repo = repos
        .iter()
        .find(|r| r.owner.eq_ignore_ascii_case(OWNER) && r.name == REPO)?;
    let rel = repo.release.as_ref()?;
    (is_newer(&rel.tag, env!("CARGO_PKG_VERSION")) && rel.assets.iter().any(|a| a.name == ASSET))
        .then(|| Available { tag: rel.tag.clone() })
}

/// Downloads the new executable and puts it in place of the running one. The caller restarts the program.
pub fn apply(client: &Client, repo: &Repo, prog: &Progress) -> Result<(), String> {
    let rel = repo.release.as_ref().ok_or("у Harbor нет релиза")?;
    let idx = rel
        .assets
        .iter()
        .position(|a| a.name == ASSET)
        .ok_or("в релизе нет Harbor.exe")?;
    let settings = Settings {
        download_dir: std::env::temp_dir().join("harbor-update"),
        delete_old: true,
        ..Settings::default()
    };
    let _ = std::fs::remove_dir_all(&settings.download_dir);
    let got = github::download(client, repo, Target::Asset(idx), &settings, prog)?;
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let old = me.with_extension("old");
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&me, &old).map_err(|e| format!("не удалось заменить файл программы: {e}"))?;
    if let Err(e) = std::fs::copy(&got.path, &me) {
        let _ = std::fs::rename(&old, &me);
        return Err(format!("не удалось записать новую версию: {e}"));
    }
    let _ = std::fs::remove_dir_all(&settings.download_dir);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::is_newer;

    #[test]
    fn version_order() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("v0.10.0", "0.9.5"));
        assert!(is_newer("1.0", "0.9.9"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("v0.1.0", "0.1.1"));
    }
}
