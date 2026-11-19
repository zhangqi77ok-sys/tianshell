# Build a Windows zip that runs without cargo flags.
# Uses the runner's default MSVC toolchain. Do not add a repo .cargo/config.toml.
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

cargo build --release --locked --bin tianshell --bin tianshell-mcp
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$stage = Join-Path $root "dist\windows\TianShell"
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force -Path $stage | Out-Null

Copy-Item (Join-Path $root "target\release\tianshell.exe") $stage
Copy-Item (Join-Path $root "target\release\tianshell-mcp.exe") $stage
Copy-Item (Join-Path $root "scripts\windows\install.ps1") $stage
Copy-Item (Join-Path $root "scripts\windows\README.txt") $stage

$zipDir = Join-Path $root "dist\windows"
$zip = Join-Path $zipDir "TianShell-windows.zip"
if (Test-Path $zip) { Remove-Item -Force $zip }
Compress-Archive -Path (Join-Path $stage "*") -DestinationPath $zip
Write-Host "Wrote $zip"
