# Builds the release artifacts into dist\:
#   Harbor-Setup.exe   the installer (the same executable; it installs itself when it is named *Setup*.exe)
#   Harbor.exe         the portable version (also what the self-updater downloads)
#   SHA256SUMS.txt     checksums of both files
#
#   powershell -ExecutionPolicy Bypass -File scripts\release.ps1             # fmt + clippy + tests + build + dist\
#   powershell -ExecutionPolicy Bypass -File scripts\release.ps1 -Publish    # ... and `gh release create`
param([switch]$Publish, [switch]$SkipChecks, [string]$NotesFile = "")

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
$tag = "v$version"
Write-Host "Harbor $tag"
if ($NotesFile -eq "") { $NotesFile = "docs\release-notes\$tag.md" }

if (-not $SkipChecks) {
    cargo fmt --check
    if ($LASTEXITCODE -ne 0) { throw "cargo fmt --check failed" }
    cargo clippy --all-targets -- -D warnings
    if ($LASTEXITCODE -ne 0) { throw "clippy failed" }
    cargo test --release
    if ($LASTEXITCODE -ne 0) { throw "tests failed" }
}
cargo build --release
if ($LASTEXITCODE -ne 0) { throw "build failed" }

$dist = Join-Path $root "dist"
if (Test-Path $dist) { Remove-Item -LiteralPath $dist -Recurse -Force }
New-Item -ItemType Directory $dist | Out-Null
Copy-Item target\release\Harbor.exe (Join-Path $dist "Harbor.exe")
# the installer is the same file under another name
Copy-Item (Join-Path $dist "Harbor.exe") (Join-Path $dist "Harbor-Setup.exe")

$lines = foreach ($f in "Harbor-Setup.exe", "Harbor.exe") {
    $h = (Get-FileHash (Join-Path $dist $f) -Algorithm SHA256).Hash.ToLower()
    "$h  $f"
}
[IO.File]::WriteAllText((Join-Path $dist "SHA256SUMS.txt"), (($lines -join "`n") + "`n"), (New-Object Text.UTF8Encoding($false)))
Get-ChildItem $dist | Format-Table Name, Length
Get-Content (Join-Path $dist "SHA256SUMS.txt")

if ($Publish) {
    $assets = Get-ChildItem $dist | ForEach-Object { $_.FullName }
    gh release create $tag @assets --title "Harbor $version" --notes-file $NotesFile
    if ($LASTEXITCODE -ne 0) { throw "gh release failed" }
}
