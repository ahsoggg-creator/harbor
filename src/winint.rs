//! Per-user install / uninstall (no administrator rights): files, shortcuts, "Apps & features" entry.

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const UNINSTALL_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\Harbor";
pub const EXE_NAME: &str = "Harbor.exe";

fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).map(PathBuf::from)
}

pub fn install_dir() -> Option<PathBuf> {
    Some(env_dir("LOCALAPPDATA")?.join("Programs").join("Harbor"))
}

pub fn installed_exe() -> Option<PathBuf> {
    Some(install_dir()?.join(EXE_NAME))
}

pub fn data_dir() -> Option<PathBuf> {
    Some(env_dir("APPDATA")?.join("Harbor"))
}

/// The installer is this same executable under a name that contains "setup".
pub fn is_setup_launch() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_lowercase()))
        .is_some_and(|s| s.contains("setup"))
}

fn run_hidden(cmd: &mut Command) -> Result<std::process::Output, String> {
    cmd.creation_flags(CREATE_NO_WINDOW).output().map_err(|e| e.to_string())
}

fn reg(args: &[&str]) -> Result<(), String> {
    let out = run_hidden(Command::new("reg.exe").args(args))?;
    out.status
        .success()
        .then_some(())
        .ok_or_else(|| String::from_utf8_lossy(&out.stderr).trim().to_string())
}

fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in data.chunks(3) {
        let n =
            (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= c.len() {
                out.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn powershell(script: &str) -> Result<(), String> {
    powershell_output(script).map(|_| ())
}

/// Runs a script and returns its stdout decoded as UTF-8 (scripts that print text set the console encoding).
pub fn powershell_output(script: &str) -> Result<String, String> {
    let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let out = run_hidden(Command::new("powershell.exe").args([
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-EncodedCommand",
        &base64(&utf16),
    ]))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn shortcut_script(exe: &Path, desktop: bool) -> String {
    let mut s = String::from("$w = New-Object -ComObject WScript.Shell\n");
    let targets = if desktop { "'Programs','Desktop'" } else { "'Programs'" };
    s += &format!(
        "foreach ($k in {targets}) {{\n  $dir = if ($k -eq 'Programs') {{ [Environment]::GetFolderPath('Programs') }} else {{ [Environment]::GetFolderPath('Desktop') }}\n  $l = $w.CreateShortcut((Join-Path $dir 'Harbor.lnk'))\n  $l.TargetPath = {exe}\n  $l.WorkingDirectory = {wd}\n  $l.Description = 'Harbor'\n  $l.Save()\n}}\n",
        exe = ps_quote(&exe.display().to_string()),
        wd = ps_quote(&exe.parent().map(|p| p.display().to_string()).unwrap_or_default()),
    );
    s
}

const REMOVE_SHORTCUTS: &str = "foreach ($d in [Environment]::GetFolderPath('Programs'), [Environment]::GetFolderPath('Desktop')) { Remove-Item -LiteralPath (Join-Path $d 'Harbor.lnk') -Force -ErrorAction SilentlyContinue }";

fn registry_entry(exe: &Path, dir: &Path) -> Result<(), String> {
    let exe_s = exe.display().to_string();
    let uninstall = format!("\"{exe_s}\" --uninstall");
    let version = env!("CARGO_PKG_VERSION");
    let size = std::fs::metadata(exe)
        .map(|m| (m.len() / 1024).to_string())
        .unwrap_or_else(|_| "0".into());
    for (name, kind, value) in [
        ("DisplayName", "REG_SZ", "Harbor"),
        ("DisplayVersion", "REG_SZ", version),
        ("Publisher", "REG_SZ", "ahsoggg"),
        ("DisplayIcon", "REG_SZ", exe_s.as_str()),
        ("InstallLocation", "REG_SZ", &dir.display().to_string()),
        ("UninstallString", "REG_SZ", uninstall.as_str()),
        ("URLInfoAbout", "REG_SZ", "https://github.com/ahsoggg-creator/harbor"),
        ("NoModify", "REG_DWORD", "1"),
        ("NoRepair", "REG_DWORD", "1"),
        ("EstimatedSize", "REG_DWORD", size.as_str()),
    ] {
        reg(&["add", UNINSTALL_KEY, "/v", name, "/t", kind, "/d", value, "/f"])?;
    }
    Ok(())
}

pub fn install(desktop_shortcut: bool) -> Result<PathBuf, String> {
    let dir = install_dir().ok_or("не найдена папка %LOCALAPPDATA%")?;
    let target = dir.join(EXE_NAME);
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("не удалось создать {}: {e}", dir.display()))?;
    // a running copy cannot be overwritten but can be renamed: the old file disappears on its next start
    let old = target.with_extension("old");
    let _ = std::fs::remove_file(&old);
    if target.exists() {
        std::fs::rename(&target, &old)
            .map_err(|e| format!("не удалось заменить установленную копию (закрой Harbor и повтори): {e}"))?;
    }
    if let Err(e) = std::fs::copy(&me, &target) {
        let _ = std::fs::rename(&old, &target);
        return Err(format!("не удалось скопировать файл: {e}"));
    }
    powershell(&shortcut_script(&target, desktop_shortcut)).map_err(|e| format!("ярлыки: {e}"))?;
    registry_entry(&target, &dir).map_err(|e| format!("запись в «Приложения и возможности»: {e}"))?;
    Ok(target)
}

pub fn uninstall(delete_settings: bool) -> Result<(), String> {
    let _ = powershell(REMOVE_SHORTCUTS);
    let _ = reg(&["delete", UNINSTALL_KEY, "/f"]);
    if delete_settings && let Some(d) = data_dir() {
        let _ = std::fs::remove_dir_all(d);
    }
    if let Some(dir) = install_dir()
        && dir.exists()
    {
        // the running executable keeps its own file locked: remove the folder a moment after this process exits
        let cmd = format!("ping -n 3 127.0.0.1 >nul & rmdir /s /q \"{}\"", dir.display());
        Command::new("cmd.exe")
            .raw_arg(format!("/C {cmd}"))
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| format!("не удалось удалить папку программы: {e}"))?;
    }
    Ok(())
}

/// Leftover of a previous self-update or upgrade.
pub fn cleanup_old() {
    if let Ok(me) = std::env::current_exe() {
        let _ = std::fs::remove_file(me.with_extension("old"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_reference() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn ps_quote_escapes_apostrophes() {
        assert_eq!(ps_quote("C:\\O'Neil"), "'C:\\O''Neil'");
    }

    #[test]
    #[ignore = "touches HKCU and the Start menu (removes them again)"]
    fn install_uninstall_roundtrip() {
        let tmp = std::env::temp_dir().join("harbor-install-test");
        let _ = std::fs::remove_dir_all(&tmp);
        // SAFETY: the only test that touches the environment.
        unsafe { std::env::set_var("LOCALAPPDATA", &tmp) };
        let exe = install(false).expect("install");
        assert!(exe.exists());
        assert!(reg(&["query", UNINSTALL_KEY, "/v", "UninstallString"]).is_ok());
        let lnk = Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-Command",
                "Test-Path (Join-Path ([Environment]::GetFolderPath('Programs')) 'Harbor.lnk')",
            ])
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&lnk.stdout).contains("True"),
            "start menu shortcut missing"
        );
        // upgrade over an existing copy
        assert!(install(false).is_ok());
        uninstall(false).expect("uninstall");
        assert!(reg(&["query", UNINSTALL_KEY]).is_err(), "registry entry still there");
        std::thread::sleep(std::time::Duration::from_secs(4));
        assert!(!install_dir().unwrap().exists(), "install dir still there");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
