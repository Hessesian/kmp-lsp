<#
.SYNOPSIS
  Install kotlin-lsp on Windows from a GitHub release.

.DESCRIPTION
  Downloads the prebuilt kotlin-lsp.exe for x86_64-pc-windows-msvc from a
  GitHub release of qdsfdhvh/kotlin-lsp, drops it into a per-user install
  directory, and adds that directory to the current user's PATH.

.PARAMETER Version
  Release tag to install (default: latest).

.PARAMETER Repo
  Repository slug owner/name (default: qdsfdhvh/kotlin-lsp).

.PARAMETER InstallDir
  Install directory (default: $env:USERPROFILE\.kotlin-lsp\bin).

.EXAMPLE
  iwr -useb https://github.com/qdsfdhvh/kotlin-lsp/releases/latest/download/install.ps1 | iex

.EXAMPLE
  $env:KOTLIN_LSP_VERSION = 'v0.14.0'; iwr -useb https://github.com/qdsfdhvh/kotlin-lsp/releases/latest/download/install.ps1 | iex
#>

[CmdletBinding()]
param(
  [string]$Version    = $env:KOTLIN_LSP_VERSION,
  [string]$Repo       = $env:KOTLIN_LSP_REPO,
  [string]$InstallDir = $env:KOTLIN_LSP_PREFIX
)

$ErrorActionPreference = 'Stop'

if (-not $Version)    { $Version    = 'latest' }
if (-not $Repo)       { $Repo       = 'qdsfdhvh/kotlin-lsp' }
if (-not $InstallDir) { $InstallDir = Join-Path $env:USERPROFILE '.kotlin-lsp\bin' }

function Write-Info($msg) { Write-Host "`e[36m::`e[0m $msg" }
function Write-Warn($msg) { Write-Host "`e[33m!`e[0m  $msg" }

# ---- detect architecture ----
$arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
switch ($arch) {
  'X64'   { $asset = 'kotlin-lsp-windows-x86_64' }
  'Arm64' { $asset = 'kotlin-lsp-windows-aarch64' }
  default { throw "Unsupported Windows architecture: $arch (expected X64 or Arm64)." }
}
Write-Info "platform: windows/$arch -> $asset"

# ---- resolve URL ----
if ($Version -eq 'latest') {
  $url = "https://github.com/$Repo/releases/latest/download/$asset.zip"
} else {
  $url = "https://github.com/$Repo/releases/download/$Version/$asset.zip"
}
Write-Info "downloading $url"

$tmp = New-Item -ItemType Directory -Path (Join-Path $env:TEMP "kotlin-lsp-install-$([guid]::NewGuid().ToString('N'))")
try {
  $zipPath = Join-Path $tmp 'asset.zip'
  Invoke-WebRequest -Uri $url -OutFile $zipPath -UseBasicParsing

  Expand-Archive -Path $zipPath -DestinationPath $tmp -Force

  # Tolerate either layout: a top-level dir matching $asset, or the exe at root.
  $binSrc = Get-ChildItem -Path $tmp -Recurse -Filter 'kotlin-lsp.exe' | Select-Object -First 1
  if (-not $binSrc) { throw "zip did not contain kotlin-lsp.exe" }

  # ---- install ----
  New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
  $dest = Join-Path $InstallDir 'kotlin-lsp.exe'
  Copy-Item -Path $binSrc.FullName -Destination $dest -Force
  Write-Info "installed -> $dest"

  # ---- verify ----
  & $dest --version | Out-Null
  if ($LASTEXITCODE -ne 0) {
    throw "binary at $dest did not run cleanly — try ``$dest --version`` to debug"
  }

  # ---- PATH ----
  $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
  $pathParts = if ($userPath) { $userPath.Split(';') } else { @() }
  if ($pathParts -notcontains $InstallDir) {
    $newPath = if ($userPath) { "$userPath;$InstallDir" } else { $InstallDir }
    [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
    Write-Info "added $InstallDir to user PATH (open a new terminal for it to take effect)"
  } else {
    Write-Info "$InstallDir already in user PATH"
  }

  Write-Host ""
  Write-Host "Next: wire up your editor -- see https://github.com/$Repo#quick-start"
}
finally {
  Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
