# install.ps1 — download and install kotlin-lsp + kotlin-jar-indexer on Windows
#
# Usage (run in PowerShell as Administrator or a user with write access to prefix):
#   iwr -useb https://raw.githubusercontent.com/Hessesian/kmp-lsp/main/install.ps1 | iex
#   iex "& { $(iwr -useb https://raw.githubusercontent.com/Hessesian/kmp-lsp/main/install.ps1) } --Version v0.19.0"
#
# Parameters:
#   -Version    pin a specific release tag (default: latest)
#   -Prefix     install directory        (default: $env:KOTLIN_LSP_PREFIX or "$env:LOCALAPPDATA\kotlin-lsp\bin")
#   -NoSidecar  skip kotlin-jar-indexer installation
[CmdletBinding()]
param(
    [string] $Version  = $env:KOTLIN_LSP_VERSION,
    [string] $Prefix   = $env:KOTLIN_LSP_PREFIX,
    [switch] $NoSidecar
)
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$Repo = "Hessesian/kmp-lsp"

# ---- helpers ----
function Write-Info  { param($msg) Write-Host ":: $msg" -ForegroundColor Cyan }
function Write-Ok    { param($msg) Write-Host "OK $msg" -ForegroundColor Green }
function Write-Err   { param($msg) Write-Error "error: $msg" }
function Write-Warn  { param($msg) Write-Host "! $msg"  -ForegroundColor Yellow }

# ---- detect architecture ----
$arch = switch ($env:PROCESSOR_ARCHITECTURE) {
    "AMD64"   { "x86_64" }
    "ARM64"   { "aarch64" }
    default   { Write-Err "unsupported architecture: $env:PROCESSOR_ARCHITECTURE"; exit 1 }
}
$Platform = "windows-$arch"
Write-Info "platform: $Platform"

# ---- resolve version ----
if (-not $Version) {
    Write-Info "resolving latest release..."
    $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" -ErrorAction Stop
    $Version = $release.tag_name
}
if (-not $Version) { Write-Err "could not resolve version from GitHub API"; exit 1 }
Write-Info "version: $Version"

# ---- install prefix ----
if (-not $Prefix) {
    $Prefix = Join-Path $env:LOCALAPPDATA "kotlin-lsp\bin"
}
New-Item -ItemType Directory -Force -Path $Prefix | Out-Null

# ---- temp dir ----
$TmpDir = Join-Path ([System.IO.Path]::GetTempPath()) "kotlin-lsp-install-$([System.IO.Path]::GetRandomFileName())"
New-Item -ItemType Directory -Force -Path $TmpDir | Out-Null
try {

# ---- download sha256sums ----
$SumsUrl = "https://github.com/$Repo/releases/download/$Version/sha256sums.txt"
Write-Info "downloading sha256sums..."
Invoke-WebRequest -Uri $SumsUrl -OutFile "$TmpDir\sha256sums.txt" -UseBasicParsing

function Get-ExpectedHash {
    param([string] $filename)
    $lines = Get-Content "$TmpDir\sha256sums.txt"
    foreach ($line in $lines) {
        $parts = $line -split '\s+'
        if ($parts.Count -ge 2 -and $parts[1].TrimStart('.').TrimStart('/') -eq $filename) {
            return $parts[0].ToUpper()
        }
    }
    return $null
}

function Install-Asset {
    param([string] $asset)
    $zipName = "$asset.zip"
    $url     = "https://github.com/$Repo/releases/download/$Version/$zipName"

    Write-Info "downloading $zipName..."
    Invoke-WebRequest -Uri $url -OutFile "$TmpDir\$zipName" -UseBasicParsing

    # Verify checksum
    $expected = Get-ExpectedHash $zipName
    if (-not $expected) { Write-Err "$zipName not found in sha256sums.txt"; exit 1 }
    $actual = (Get-FileHash "$TmpDir\$zipName" -Algorithm SHA256).Hash.ToUpper()
    if ($actual -ne $expected) {
        Write-Err "checksum mismatch for $zipName`n  expected: $expected`n  got: $actual"
        exit 1
    }
    Write-Ok "checksum verified"

    Expand-Archive -Path "$TmpDir\$zipName" -DestinationPath "$TmpDir\$asset" -Force
}

Install-Asset "kotlin-lsp-$Platform"
if (-not $NoSidecar) {
    Install-Asset "kotlin-jar-indexer-$Platform"
}

# ---- copy binaries to prefix ----
function Copy-Binary {
    param([string] $name, [string] $assetDir)
    $src = Join-Path $TmpDir "$assetDir\$name.exe"
    if (-not (Test-Path $src)) {
        # some archives place binaries at the root of the zip
        $src = Join-Path $TmpDir "$assetDir\$name"
        if (-not (Test-Path $src)) {
            Write-Err "binary '$name' not found in archive under $assetDir"
            exit 1
        }
    }
    $dst = Join-Path $Prefix "$name.exe"
    Copy-Item -Force -Path $src -Destination $dst
    Write-Ok "$name.exe -> $dst"
}

Copy-Binary "kotlin-lsp"         "kotlin-lsp-$Platform"
if (-not $NoSidecar) {
    Copy-Binary "kotlin-jar-indexer" "kotlin-jar-indexer-$Platform"
}

} finally {
    Remove-Item -Recurse -Force $TmpDir -ErrorAction SilentlyContinue
}

# ---- verify ----
$binary = Join-Path $Prefix "kotlin-lsp.exe"
try {
    $ver = & $binary --version 2>&1
    Write-Info $ver
} catch {
    Write-Warn "installed but could not run --version: $_"
}

# ---- PATH hint ----
$machinePath = [System.Environment]::GetEnvironmentVariable("PATH", "User")
if ($machinePath -notlike "*$Prefix*") {
    Write-Warn "$Prefix is not in your PATH."
    Write-Host "  To add it permanently, run in a new terminal:"
    Write-Host "  [System.Environment]::SetEnvironmentVariable('PATH', `"$Prefix;`$([System.Environment]::GetEnvironmentVariable('PATH','User'))`", 'User')"
    Write-Host ""
}

Write-Host ""
Write-Host "Next: wire up your editor — see docs at"
Write-Host "  https://github.com/$Repo#quick-start"
Write-Host ""
