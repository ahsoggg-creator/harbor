# Harbor

[English](README.en.md) · [Русский](README.md)

All your GitHub projects in one Windows window: the repository list updates itself, and programs from releases are
downloaded, launched, updated and removed with one button. Think of an app store for your own repositories.

![Repository list](docs/screenshots/01-repos.png)

*The screenshot shows the author's public repositories; private ones are hidden.*

## Features

- **Automatic list.** Your repositories and their latest releases are fetched at start-up and on a timer (every
  10 minutes by default, `F5` for now). New repositories and releases are written to the log. Requests are conditional
  (ETag), so they barely touch the GitHub API rate limit.
- **Download.** Progress and cancel; when a release has several files you pick one (the installer by default). If the
  release ships `SHA256SUMS.txt` the hash is verified and a mismatching file is never kept. Repositories without
  releases offer "Download source" (zip).
- **Run / Folder / Delete.** "Delete" removes the downloaded files (with confirmation). **"Remove from list"** only
  hides the card and leaves files alone; bring it back with the "Hidden" filter.
- **Updates.** An "update available" badge, "Update" and "Update all", optional auto-download; old versions are removed.
- **Private repositories.** The token comes from `gh auth token` or is entered by hand in Settings. Without a token
  only public repositories work, with a 60 requests/hour limit.
- **Comfort.** Search (`Ctrl+F`), filters (with release, downloaded, has update, favourites, private, hidden), sorting,
  tiles or list, favourites, release notes, adding any public repository by URL, an event log, Markdown export.
- **Per-user installer** (no admin rights) and self-update: Harbor is one of the listed repositories, so its own new
  release is found by the same sync.

## Install

1. Download `Harbor-Setup.exe` from [Releases](https://github.com/ahsoggg-creator/harbor/releases) and run it. It
   installs to `%LOCALAPPDATA%\Programs\Harbor` for your account and adds a Start menu shortcut.
2. Or take the portable `Harbor.exe`, which installs nothing.
3. Enter your GitHub login in Settings.

The files are **unsigned**: SmartScreen may warn ("More info" → "Run anyway"). Checksums are in `SHA256SUMS.txt`.
Uninstall from Settings → Apps or with `Harbor.exe --uninstall`.

## Where things live

| What | Where |
| --- | --- |
| Settings, list cache, log | `%APPDATA%\Harbor\state.json` |
| Downloaded programs | `Downloads\Harbor\<repo>\<version>\` (the folder is configurable) |

A token entered by hand is stored in `state.json` in plain text; a token taken from `gh` is never saved.

## Build

```powershell
cargo build --release          # target\release\Harbor.exe
cargo test                     # unit tests
cargo test -- --ignored        # network (downloads ~17 MB) and installer check (touches HKCU and the Start menu)
Harbor.exe --shot out.png      # window screenshot (settings | log for another tab)
```

Releases are built by GitHub Actions on a `v*` tag (`scripts\release.ps1`).

## License

MIT or Apache-2.0, at your option.
