# TianShell Installer Build Script
# ---------------------------------------------
$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$ProjectRoot = Split-Path -Parent $ScriptDir
Set-Location $ProjectRoot

Write-Host "=========================================" -ForegroundColor Cyan
Write-Host " Building TianShell Release & Installer " -ForegroundColor Cyan
Write-Host "=========================================" -ForegroundColor Cyan

# 1. 编译 Release 二进制文件
Write-Host "[1/3] Compiling release binaries via cargo..." -ForegroundColor Yellow
cargo build --release

$AppExe = Join-Path $ProjectRoot "target\release\tianshell.exe"
$McpExe = Join-Path $ProjectRoot "target\release\tianshell-mcp.exe"
$LoaderDll = Join-Path $ProjectRoot "target\release\WebView2Loader.dll"

if (-not (Test-Path $AppExe)) {
    throw "Build failed: tianshell.exe not found at $AppExe"
}
if (-not (Test-Path $McpExe)) {
    throw "Build failed: tianshell-mcp.exe not found at $McpExe"
}

# 确保 WebView2Loader.dll 存在于 release 目录
if (-not (Test-Path $LoaderDll)) {
    $DllCandidates = Get-ChildItem -Path (Join-Path $ProjectRoot "target\release\build\webview2-com-sys") -Filter "WebView2Loader.dll" -Recurse | Where-Object { $_.FullName -like "*\x64\*" }
    if ($DllCandidates) {
        Copy-Item $DllCandidates[0].FullName $LoaderDll -Force
    } else {
        $DebugDll = Join-Path $ProjectRoot "target\debug\WebView2Loader.dll"
        if (Test-Path $DebugDll) {
            Copy-Item $DebugDll $LoaderDll -Force
        }
    }
}

if (-not (Test-Path $LoaderDll)) {
    throw "Build failed: WebView2Loader.dll not found."
}

Write-Host "  -> tianshell.exe size: $((Get-Item $AppExe).Length / 1MB | ForEach-Object { '{0:N2} MB' -f $_ })" -ForegroundColor Green
Write-Host "  -> tianshell-mcp.exe size: $((Get-Item $McpExe).Length / 1MB | ForEach-Object { '{0:N2} MB' -f $_ })" -ForegroundColor Green
Write-Host "  -> WebView2Loader.dll size: $((Get-Item $LoaderDll).Length / 1KB | ForEach-Object { '{0:N2} KB' -f $_ })" -ForegroundColor Green

# 2. 定位 makensis
Write-Host "[2/3] Locating NSIS compiler (makensis.exe)..." -ForegroundColor Yellow
$NsisCandidates = @(
    "makensis",
    "C:\Users\Admin\AppData\Local\NSIS\makensis.exe",
    "C:\Program Files (x86)\NSIS\makensis.exe",
    "C:\Program Files\NSIS\makensis.exe"
)

$Makensis = $null
foreach ($c in $NsisCandidates) {
    if (Get-Command $c -ErrorAction SilentlyContinue) {
        $Makensis = $c
        break
    }
    if (Test-Path $c) {
        $Makensis = $c
        break
    }
}

if (-not $Makensis) {
    throw "makensis.exe not found. Please install NSIS or add to PATH."
}
Write-Host "  -> Found makensis at: $Makensis" -ForegroundColor Green

# 3. 执行 NSIS 打包
Write-Host "[3/3] Compiling installer..." -ForegroundColor Yellow
$NsiPath = Join-Path $ProjectRoot "installer\tianshell.nsi"
& $Makensis /INPUTCHARSET UTF8 $NsiPath

$CargoToml = Get-Content (Join-Path $ProjectRoot "Cargo.toml") -Raw
if ($CargoToml -match 'version\s*=\s*"([^"]+)"') {
    $Version = $matches[1]
} else {
    $Version = "0.0.1"
}

$InstallerPath = Join-Path $ProjectRoot "target\TianShell-Setup-$Version.exe"
if (-not (Test-Path $InstallerPath)) {
    throw "Installer generation failed: $InstallerPath not found"
}

$InstallerSize = (Get-Item $InstallerPath).Length
Write-Host "=========================================" -ForegroundColor Green
Write-Host " SUCCESS! Installer generated: " -ForegroundColor Green
Write-Host " File: $InstallerPath" -ForegroundColor White
Write-Host " Size: $('{0:N2} MB' -f ($InstallerSize / 1MB)) ($InstallerSize bytes)" -ForegroundColor White
Write-Host "=========================================" -ForegroundColor Green
