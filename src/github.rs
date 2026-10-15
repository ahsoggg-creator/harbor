use crate::model::{Asset, Installed, Release, Repo, SOURCE_TAG, Settings, now, repo_dir, sanitize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const API: &str = "https://api.github.com";

type ReleaseResult = (usize, Result<Option<Release>, String>);

pub type EtagCache = Arc<Mutex<HashMap<String, (String, String)>>>;

pub struct Client {
    api: ureq::Agent,
    dl: ureq::Agent,
    pub token: Option<String>,
    cache: EtagCache,
    pub rate_remaining: Arc<AtomicI64>,
}

fn agent(timeout: Duration) -> ureq::Agent {
    let tls = ureq::tls::TlsConfig::builder()
        .provider(ureq::tls::TlsProvider::NativeTls)
        .build();
    ureq::Agent::config_builder()
        .tls_config(tls)
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_global(Some(timeout))
        .build()
        .into()
}

impl Client {
    pub fn new(token: Option<String>, cache: EtagCache, rate: Arc<AtomicI64>) -> Self {
        Self {
            api: agent(Duration::from_secs(40)),
            dl: agent(Duration::from_secs(60 * 60)),
            token,
            cache,
            rate_remaining: rate,
        }
    }

    fn authed(
        &self,
        req: ureq::RequestBuilder<ureq::typestate::WithoutBody>,
        url: &str,
    ) -> ureq::RequestBuilder<ureq::typestate::WithoutBody> {
        let req = req
            .header("User-Agent", "Harbor")
            .header("X-GitHub-Api-Version", "2022-11-28");
        match &self.token {
            Some(t) if url.starts_with(API) => req.header("Authorization", format!("Bearer {t}")),
            _ => req,
        }
    }

    /// `Ok(None)` is a 404. Conditional requests keep repeated syncs off the rate limit.
    fn get_json(&self, url: &str) -> Result<Option<Value>, String> {
        let cached = self.cache.lock().ok().and_then(|c| c.get(url).cloned());
        let mut req = self
            .authed(self.api.get(url), url)
            .header("Accept", "application/vnd.github+json");
        if let Some((etag, _)) = &cached {
            req = req.header("If-None-Match", etag.as_str());
        }
        let mut resp = req.call().map_err(|e| e.to_string())?;
        if let Some(n) = resp
            .headers()
            .get("x-ratelimit-remaining")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<i64>().ok())
        {
            self.rate_remaining.store(n, Ordering::Relaxed);
        }
        let status = resp.status().as_u16();
        let body = match status {
            304 => cached.map(|(_, b)| b).ok_or("HTTP 304 без кэша")?,
            404 => return Ok(None),
            200..=299 => {
                let etag = resp
                    .headers()
                    .get("etag")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);
                let body = resp.body_mut().read_to_string().map_err(|e| e.to_string())?;
                if let (Some(etag), Ok(mut c)) = (etag, self.cache.lock()) {
                    c.insert(url.to_string(), (etag, body.clone()));
                }
                body
            }
            401 => return Err("HTTP 401: токен недействителен".into()),
            403 | 429 => {
                return Err(if self.rate_remaining.load(Ordering::Relaxed) == 0 {
                    "превышен лимит GitHub API — добавь токен в настройках или подожди".into()
                } else {
                    format!("HTTP {status}")
                });
            }
            s => return Err(format!("HTTP {s}")),
        };
        serde_json::from_str(&body).map(Some).map_err(|e| e.to_string())
    }
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn parse_repo(v: &Value, manual: bool) -> Option<Repo> {
    Some(Repo {
        owner: v.pointer("/owner/login")?.as_str()?.to_string(),
        name: s(v, "name"),
        description: s(v, "description"),
        language: v.get("language").and_then(Value::as_str).map(str::to_string),
        stars: v.get("stargazers_count").and_then(Value::as_u64).unwrap_or(0) as u32,
        private: v.get("private").and_then(Value::as_bool).unwrap_or(false),
        archived: v.get("archived").and_then(Value::as_bool).unwrap_or(false),
        fork: v.get("fork").and_then(Value::as_bool).unwrap_or(false),
        pushed_at: s(v, "pushed_at"),
        html_url: s(v, "html_url"),
        default_branch: if s(v, "default_branch").is_empty() {
            "main".into()
        } else {
            s(v, "default_branch")
        },
        release: None,
        manual,
    })
    .filter(|r| !r.name.is_empty())
}

fn parse_release(v: &Value) -> Release {
    Release {
        tag: s(v, "tag_name"),
        name: s(v, "name"),
        published_at: s(v, "published_at"),
        notes: s(v, "body"),
        assets: v
            .get("assets")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|x| Asset {
                        name: s(x, "name"),
                        url: s(x, "browser_download_url"),
                        api_url: s(x, "url"),
                        size: x.get("size").and_then(Value::as_u64).unwrap_or(0),
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

pub struct SyncResult {
    pub repos: Vec<Repo>,
    pub warnings: Vec<String>,
    pub token_source: &'static str,
}

/// Token from settings, else from the `gh` CLI (read-only use: listing repos and releases).
pub fn resolve_token(settings: &Settings) -> (Option<String>, &'static str) {
    if !settings.token.trim().is_empty() {
        return (Some(settings.token.trim().to_string()), "вручную");
    }
    if settings.use_gh_token
        && let Some(t) = gh_token()
    {
        return (Some(t), "gh CLI");
    }
    (None, "нет")
}

fn gh_token() -> Option<String> {
    use std::os::windows::process::CommandExt;
    let candidates = ["gh".to_string(), r"C:\Program Files\GitHub CLI\gh.exe".to_string()];
    candidates.iter().find_map(|exe| {
        let out = std::process::Command::new(exe)
            .args(["auth", "token"])
            .creation_flags(0x0800_0000)
            .output()
            .ok()?;
        let t = String::from_utf8(out.stdout).ok()?.trim().to_string();
        (out.status.success() && !t.is_empty()).then_some(t)
    })
}

pub fn sync(
    client: &Client,
    settings: &Settings,
    manual: &BTreeSet<String>,
    prev: &[Repo],
) -> Result<SyncResult, String> {
    let user = settings.username.trim();
    if user.is_empty() {
        return Err("не указан GitHub-логин (Настройки)".into());
    }
    let own_listing = client.token.is_some() && settings.include_private;
    let mut repos: Vec<Repo> = Vec::new();
    for page in 1..=10 {
        let url = if own_listing {
            format!("{API}/user/repos?per_page=100&affiliation=owner&sort=pushed&page={page}")
        } else {
            format!("{API}/users/{user}/repos?per_page=100&sort=pushed&page={page}")
        };
        let Some(Value::Array(items)) = client.get_json(&url)? else {
            break;
        };
        let n = items.len();
        repos.extend(
            items
                .iter()
                .filter_map(|v| parse_repo(v, false))
                .filter(|r| r.owner.eq_ignore_ascii_case(user)),
        );
        if n < 100 {
            break;
        }
    }
    repos.retain(|r| (settings.include_forks || !r.fork) && (settings.include_archived || !r.archived));

    let mut warnings = Vec::new();
    for key in manual {
        if repos.iter().any(|r| r.key().eq_ignore_ascii_case(key)) {
            continue;
        }
        match client.get_json(&format!("{API}/repos/{key}")) {
            Ok(Some(v)) => repos.extend(parse_repo(&v, true)),
            Ok(None) => warnings.push(format!("{key}: репозиторий не найден или приватный (нужен токен)")),
            Err(e) => warnings.push(format!("{key}: {e}")),
        }
    }

    let next = AtomicUsize::new(0);
    let found: Mutex<Vec<ReleaseResult>> = Mutex::new(Vec::new());
    std::thread::scope(|sc| {
        for _ in 0..6 {
            sc.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(r) = repos.get(i) else { break };
                    let res = client
                        .get_json(&format!("{API}/repos/{}/releases/latest", r.key()))
                        .map(|o| o.map(|v| parse_release(&v)));
                    if let Ok(mut f) = found.lock() {
                        f.push((i, res));
                    }
                }
            });
        }
    });
    for (i, res) in found.into_inner().unwrap_or_default() {
        match res {
            Ok(rel) => repos[i].release = rel,
            Err(e) => {
                warnings.push(format!("{}: релиз не получен ({e})", repos[i].key()));
                repos[i].release = prev
                    .iter()
                    .find(|p| p.key() == repos[i].key())
                    .and_then(|p| p.release.clone());
            }
        }
    }
    let token_source = if client.token.is_some() {
        if settings.token.trim().is_empty() {
            "gh CLI"
        } else {
            "вручную"
        }
    } else {
        "нет"
    };
    Ok(SyncResult {
        repos,
        warnings,
        token_source,
    })
}

#[derive(Default)]
pub struct Progress {
    pub done: AtomicU64,
    pub total: AtomicU64,
    pub cancel: AtomicBool,
    pub stage: Mutex<String>,
}

impl Progress {
    fn stage(&self, text: &str) {
        if let Ok(mut s) = self.stage.lock() {
            *s = text.to_string();
        }
    }
    pub fn stage_text(&self) -> String {
        self.stage.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

pub enum Target {
    Asset(usize),
    Source,
}

/// Downloads into `<dir>/<repo>/<tag>/<file>`, verifying SHA-256 when the release ships a checksum file.
pub fn download(
    client: &Client,
    repo: &Repo,
    target: Target,
    settings: &Settings,
    prog: &Progress,
) -> Result<Installed, String> {
    let base = repo_dir(&settings.download_dir, &repo.name);
    let (tag, file, url, size_hint, expected) = match target {
        Target::Asset(i) => {
            let rel = repo.release.as_ref().ok_or("у репозитория нет релиза")?;
            let a = rel.assets.get(i).ok_or("файл релиза не найден")?;
            let private_api = repo.private && client.token.is_some();
            let url = if private_api { a.api_url.clone() } else { a.url.clone() };
            prog.stage("проверка контрольной суммы…");
            let expected = rel
                .checksum_asset()
                .and_then(|c| fetch_checksum(client, c, &a.name, repo.private));
            (rel.tag.clone(), a.name.clone(), url, a.size, expected)
        }
        Target::Source => {
            let b = &repo.default_branch;
            let url = if repo.private && client.token.is_some() {
                format!("{API}/repos/{}/zipball/{b}", repo.key())
            } else {
                format!("https://github.com/{}/archive/refs/heads/{b}.zip", repo.key())
            };
            (SOURCE_TAG.to_string(), format!("{}-{}.zip", repo.name, b), url, 0, None)
        }
    };
    let dir = base.join(sanitize(&tag));
    std::fs::create_dir_all(&dir).map_err(|e| format!("папка загрузок: {e}"))?;
    let dest = dir.join(sanitize(&file));
    let part = dest.with_extension("part");
    prog.stage("скачивание…");
    prog.total.store(size_hint, Ordering::Relaxed);
    prog.done.store(0, Ordering::Relaxed);
    let hash = fetch_to_file(client, &url, &part, prog).inspect_err(|_| {
        let _ = std::fs::remove_file(&part);
    })?;
    if let Some(exp) = expected
        && !exp.eq_ignore_ascii_case(&hash)
    {
        let _ = std::fs::remove_file(&part);
        return Err(format!(
            "контрольная сумма не совпала (ожидалось {exp}, получено {hash})"
        ));
    }
    let _ = std::fs::remove_file(&dest);
    std::fs::rename(&part, &dest).map_err(|e| format!("не удалось сохранить файл: {e}"))?;
    if settings.delete_old {
        remove_other_versions(&base, &sanitize(&tag));
    }
    let size = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
    Ok(Installed {
        tag,
        asset: file,
        path: dest,
        size,
        at: now(),
    })
}

fn remove_other_versions(base: &Path, keep: &str) {
    let Ok(rd) = std::fs::read_dir(base) else { return };
    for e in rd.flatten() {
        if e.file_name().to_string_lossy() != keep && e.path().is_dir() {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

fn fetch_checksum(client: &Client, sums: &Asset, file: &str, private: bool) -> Option<String> {
    let url = if private && client.token.is_some() {
        &sums.api_url
    } else {
        &sums.url
    };
    let mut resp = request(client, url).ok()?;
    let text = resp.body_mut().read_to_string().ok()?;
    text.lines()
        .find_map(|l| {
            let mut it = l.split_whitespace();
            let (h, n) = (it.next()?, it.next()?.trim_start_matches('*'));
            (n == file && h.len() == 64).then(|| h.to_lowercase())
        })
        .or_else(|| {
            // a bare `<file>.sha256` holds just the hash
            let h = text.split_whitespace().next()?;
            (sums.name.to_lowercase().ends_with(".sha256") && h.len() == 64).then(|| h.to_lowercase())
        })
}

fn request(client: &Client, url: &str) -> Result<ureq::http::Response<ureq::Body>, String> {
    let req = client
        .authed(client.dl.get(url), url)
        .header("Accept", "application/octet-stream");
    let resp = req.call().map_err(|e| e.to_string())?;
    match resp.status().as_u16() {
        200..=299 => Ok(resp),
        404 => Err("файл не найден (404)".into()),
        s => Err(format!("HTTP {s}")),
    }
}

fn fetch_to_file(client: &Client, url: &str, part: &PathBuf, prog: &Progress) -> Result<String, String> {
    let resp = request(client, url)?;
    if let Some(n) = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
    {
        prog.total.store(n, Ordering::Relaxed);
    }
    let mut reader = resp.into_body().into_reader();
    let mut out = std::fs::File::create(part).map_err(|e| format!("не удалось создать файл: {e}"))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        if prog.cancel.load(Ordering::Relaxed) {
            return Err("отменено".into());
        }
        let n = reader.read(&mut buf).map_err(|e| format!("обрыв загрузки: {e}"))?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n]).map_err(|e| format!("запись на диск: {e}"))?;
        hasher.update(&buf[..n]);
        prog.done.fetch_add(n as u64, Ordering::Relaxed);
    }
    out.flush().map_err(|e| e.to_string())?;
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> Client {
        let (token, _) = resolve_token(&Settings::default());
        Client::new(token, Arc::default(), Arc::new(AtomicI64::new(-1)))
    }

    #[test]
    #[ignore = "network"]
    fn live_sync_lists_public_repos_with_releases() {
        let t = std::time::Instant::now();
        let r = sync(&client(), &Settings::default(), &BTreeSet::new(), &[]).expect("sync");
        eprintln!(
            "{} repos in {:?}, warnings: {:?}",
            r.repos.len(),
            t.elapsed(),
            r.warnings
        );
        assert!(r.repos.iter().any(|x| x.name == "snapforge" && x.release.is_some()));
    }

    #[test]
    #[ignore = "network, downloads ~17 MB"]
    fn live_download_verifies_checksum() {
        let c = client();
        let settings = Settings {
            download_dir: std::env::temp_dir().join("harbor-test-dl"),
            ..Settings::default()
        };
        let _ = std::fs::remove_dir_all(&settings.download_dir);
        let r = sync(&c, &settings, &BTreeSet::new(), &[]).expect("sync");
        let repo = r.repos.iter().find(|x| x.name == "spacetree").expect("spacetree");
        let rel = repo.release.as_ref().expect("release");
        assert!(rel.checksum_asset().is_some(), "release should ship SHA256SUMS");
        let i = rel.best_asset().unwrap();
        let prog = Progress::default();
        let inst = download(&c, repo, Target::Asset(i), &settings, &prog).expect("download");
        assert!(inst.path.exists() && inst.size == rel.assets[i].size);
        // a second version folder is cleaned up when delete_old is on
        std::fs::create_dir_all(inst.path.parent().unwrap().parent().unwrap().join("v0.0.1")).unwrap();
        download(&c, repo, Target::Asset(i), &settings, &prog).expect("redownload");
        assert!(!settings.download_dir.join("spacetree").join("v0.0.1").exists());
        std::fs::remove_dir_all(&settings.download_dir).unwrap();
    }
}
