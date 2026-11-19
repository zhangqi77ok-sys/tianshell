# Copy TianShell next to this script into the current user's local app folder
# and add a Start Menu shortcut. No machine-specific toolchain paths.
$ErrorActionPreference = "Stop"
$here = $PSScriptRoot
$dest = Join-Path $env:LOCALAPPDATA "TianShell"
New-Item -ItemType Directory -Force -Path $dest | Out-Null
Copy-Item (Join-Path $here "tianshell.exe") $dest -Force
Copy-Item (Join-Path $here "tianshell-mcp.exe") $dest -Force

$startMenu = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs"
New-Item -ItemType Directory -Force -Path $startMenu | Out-Null
$shell = New-Object -ComObject WScript.Shell
$shortcut = $shell.CreateShortcut((Join-Path $startMenu "TianShell.lnk"))
$shortcut.TargetPath = Join-Path $dest "tianshell.exe"
$shortcut.WorkingDirectory = $dest
$shortcut.Description = "TianShell SSH workbench"
$shortcut.Save()

Write-Host "Installed to $dest"
Write-Host "Start Menu shortcut: TianShell"
Write-Host "Leave the console window open. Closing it stops TianShell."
