<#
.SYNOPSIS
    Builds the Omnilogue Server tray app and its Windows installer for one
    upstream Omnilogue release.

.DESCRIPTION
    1. Checks out jhlee0409/omnilogue at the release tag (or uses -UpstreamDir).
    2. Builds the web UI and the headless server (cargo --features webui-server).
    3. Copies the server into server-tray as the Tauri sidecar.
    4. Builds the tray app and NSIS installer with the upstream version number.
    5. Puts the installer, its signature and latest-server-tray.json in dist\.

    Signing: set TAURI_SIGNING_PRIVATE_KEY (and TAURI_SIGNING_PRIVATE_KEY_PASSWORD)
    to produce update signatures. Without a key the installer is still built,
    but no update manifest is written.

    Works in Windows PowerShell 5.1 and PowerShell 7. Needs git, Node.js,
    pnpm, Rust (MSVC) and the WebView2 runtime that Windows 10/11 already has.

.EXAMPLE
    ./scripts/build.ps1                       # latest upstream release
.EXAMPLE
    ./scripts/build.ps1 -Tag v2.1.1 -UpstreamDir C:\src\omnilogue -Repo you/omnilogue-server-windows
#>
[CmdletBinding()]
param(
    # Upstream release tag, e.g. v2.1.1. Defaults to the latest release.
    [string]$Tag,
    # Existing checkout of the upstream repo at -Tag. Default: a clone under
    # %LOCALAPPDATA%\omnilogue-server-build\<tag>.
    [string]$UpstreamDir,
    # owner/name of the GitHub repo that publishes the installers; the tray
    # app looks for updates there.
    [string]$Repo = $(if ($env:GITHUB_REPOSITORY) { $env:GITHUB_REPOSITORY } else { 'OWNER/omnilogue-server-windows' }),
    [string]$UpstreamRepo = 'jhlee0409/omnilogue',
    # Reuse the server exe already in server-tray\src-tauri\binaries.
    [switch]$SkipServerBuild
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 3

$root     = Split-Path $PSScriptRoot -Parent
$trayDir  = Join-Path $root 'server-tray'
$tauriDir = Join-Path $trayDir 'src-tauri'
$distDir  = Join-Path $root 'dist'

function Invoke-Checked {
    param([string]$What, [scriptblock]$Command)
    Write-Host "==> $What" -ForegroundColor Cyan
    # Windows PowerShell turns redirected native stderr (cargo's progress
    # output) into errors, which 'Stop' would make fatal. Rely on exit codes.
    $ErrorActionPreference = 'Continue'
    & $Command
    if ($LASTEXITCODE -ne 0) { throw "$What failed (exit code $LASTEXITCODE)" }
}

# pnpm from PATH, or the version pinned in server-tray\package.json via npx.
$pnpmCommand = Get-Command pnpm -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
$pnpmVersion = ((Get-Content (Join-Path $trayDir 'package.json') -Raw | ConvertFrom-Json).packageManager -split '@')[1]
function Invoke-Pnpm {
    # A plain array, not @args: splatting into npx.ps1 drops the arguments.
    $pnpmArgs = @($args)
    if ($pnpmCommand) { & $pnpmCommand.Source $pnpmArgs } else { & npx.cmd --yes "pnpm@$pnpmVersion" $pnpmArgs }
}

function Write-Utf8NoBom {
    param([string]$Path, [string]$Text)
    [IO.File]::WriteAllText($Path, $Text, (New-Object Text.UTF8Encoding $false))
}

# --- Which release ---------------------------------------------------------
if (-not $Tag) {
    $headers = @{ 'User-Agent' = 'omnilogue-server-windows-build' }
    if ($env:GITHUB_TOKEN) { $headers.Authorization = "Bearer $env:GITHUB_TOKEN" }
    $Tag = (Invoke-RestMethod "https://api.github.com/repos/$UpstreamRepo/releases/latest" -Headers $headers).tag_name
}
$version = $Tag -replace '^v', ''
# NSIS needs a plain numeric version, and the updater compares semver.
if ($version -notmatch '^\d+\.\d+\.\d+$') {
    throw "Upstream tag '$Tag' is not a plain x.y.z release; refusing to build it."
}
if ($Repo -notmatch '^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$') { throw "-Repo must look like owner/name, got '$Repo'." }
Write-Host "Building Omnilogue Server $version (upstream $UpstreamRepo $Tag), updates from $Repo"

# --- Upstream server -------------------------------------------------------
$triple = ((& rustc -vV) | Select-String '^host: (.+)$').Matches[0].Groups[1].Value
$sidecar = Join-Path $tauriDir "binaries\cchv-server-$triple.exe"

if ($SkipServerBuild) {
    if (-not (Test-Path $sidecar)) { throw "-SkipServerBuild given but $sidecar does not exist." }
} else {
    if (-not $UpstreamDir) {
        $UpstreamDir = Join-Path $env:LOCALAPPDATA "omnilogue-server-build\$Tag"
    }
    if (-not (Test-Path (Join-Path $UpstreamDir 'package.json'))) {
        Invoke-Checked "Clone $UpstreamRepo $Tag" {
            git clone --depth 1 --branch $Tag "https://github.com/$UpstreamRepo.git" $UpstreamDir
        }
    }
    Push-Location $UpstreamDir
    try {
        Invoke-Checked 'Install web UI dependencies' { Invoke-Pnpm install --frozen-lockfile }
        Invoke-Checked 'Type-check web UI' { Invoke-Pnpm exec tsc --build . }
        Invoke-Checked 'Build web UI' { Invoke-Pnpm exec vite build }
        # The web UI in dist\ is embedded into the server exe.
        Invoke-Checked 'Build server' {
            cargo build --release --features webui-server --manifest-path src-tauri/Cargo.toml
        }
    } finally {
        Pop-Location
    }
    $built = Join-Path $UpstreamDir 'src-tauri\target\release\claude-code-history-viewer.exe'
    New-Item -ItemType Directory -Force (Split-Path $sidecar) | Out-Null
    Copy-Item $built $sidecar -Force
    Copy-Item (Join-Path $UpstreamDir 'LICENSE') (Join-Path $tauriDir 'resources\OMNILOGUE-LICENSE.txt') -Force
}

# --- Tray app and installer ------------------------------------------------
# Version, update address and signing are passed as a config overlay, so the
# committed tauri.conf.json stays unchanged.
$signing = [bool]$env:TAURI_SIGNING_PRIVATE_KEY
if (-not $signing) {
    Write-Warning 'TAURI_SIGNING_PRIVATE_KEY is not set: building without update signatures.'
}
$overlay = [ordered]@{
    version = $version
    bundle  = @{ createUpdaterArtifacts = $signing }
    plugins = @{ updater = @{ endpoints = @("https://github.com/$Repo/releases/latest/download/latest-server-tray.json") } }
}
$overlayPath = Join-Path $tauriDir 'target\release-overlay.json'
New-Item -ItemType Directory -Force (Split-Path $overlayPath) | Out-Null
Write-Utf8NoBom $overlayPath ($overlay | ConvertTo-Json -Depth 5)

Push-Location $trayDir
try {
    Invoke-Checked 'Install Tauri CLI' { Invoke-Pnpm install --frozen-lockfile }
    Invoke-Checked 'Build tray app and installer' { Invoke-Pnpm tauri build --config $overlayPath }
} finally {
    Pop-Location
}

# --- Collect outputs -------------------------------------------------------
$nsisDir = Join-Path $tauriDir 'target\release\bundle\nsis'
$setup = Get-ChildItem $nsisDir -Filter "*_${version}_*-setup.exe" | Select-Object -First 1
if (-not $setup) { throw "No installer for $version found in $nsisDir." }

if (Test-Path $distDir) { Remove-Item $distDir -Recurse -Force }
New-Item -ItemType Directory $distDir | Out-Null
$assetName = "OmnilogueServer_${version}_x64-setup.exe"
Copy-Item $setup.FullName (Join-Path $distDir $assetName)

if ($signing) {
    $signature = (Get-Content "$($setup.FullName).sig" -Raw).Trim()
    Copy-Item "$($setup.FullName).sig" (Join-Path $distDir "$assetName.sig")
    $manifest = [ordered]@{
        version   = $version
        notes     = "Omnilogue $Tag. Release notes: https://github.com/$UpstreamRepo/releases/tag/$Tag"
        pub_date  = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
        platforms = [ordered]@{
            'windows-x86_64' = [ordered]@{
                signature = $signature
                url       = "https://github.com/$Repo/releases/download/$Tag/$assetName"
            }
        }
    }
    Write-Utf8NoBom (Join-Path $distDir 'latest-server-tray.json') ($manifest | ConvertTo-Json -Depth 5)
}

Write-Host ''
Write-Host "Done. Files in $distDir :" -ForegroundColor Green
Get-ChildItem $distDir | ForEach-Object { Write-Host ("  {0}  ({1:N1} MB)" -f $_.Name, ($_.Length / 1MB)) }
