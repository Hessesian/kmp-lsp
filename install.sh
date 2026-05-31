#!/usr/bin/env bash
# install.sh — download and install kotlin-lsp + kotlin-jar-indexer (native sidecar)
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/Hessesian/kotlin-lsp/main/install.sh | bash
#   curl -fsSL https://raw.githubusercontent.com/Hessesian/kotlin-lsp/main/install.sh | bash -s -- --version v0.19.0
#
# Environment variables:
#   KOTLIN_LSP_VERSION   pin a version (e.g. v0.19.0). Default: latest release.
#   KOTLIN_LSP_PREFIX    install directory. Default: $HOME/.local/bin
#
# For Windows use install.ps1:
#   iwr -useb https://raw.githubusercontent.com/Hessesian/kotlin-lsp/main/install.ps1 | iex

set -euo pipefail

REPO="Hessesian/kotlin-lsp"
PREFIX="${KOTLIN_LSP_PREFIX:-$HOME/.local/bin}"

err()  { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }
info() { printf '\033[36m::\033[0m %s\n' "$*"; }
ok()   { printf '\033[32m✓\033[0m %s\n' "$*"; }

# Parse --version flag (also honoured via KOTLIN_LSP_VERSION env var).
VERSION="${KOTLIN_LSP_VERSION:-}"
while [ $# -gt 0 ]; do
  case "$1" in
    --version)
      [ -n "${2:-}" ] || err "--version requires a value (e.g. --version v0.19.0)"
      VERSION="$2"; shift 2 ;;
    *) err "unknown argument: $1" ;;
  esac
done

# ---- detect platform ----
uname_s="$(uname -s)"
uname_m="$(uname -m)"
case "$uname_s" in
  Linux)  os="linux" ;;
  Darwin) os="darwin" ;;
  *) err "unsupported OS: $uname_s (use install.ps1 on Windows)" ;;
esac
case "$uname_m" in
  x86_64|amd64)    arch="x86_64" ;;
  aarch64|arm64)   arch="aarch64" ;;
  *) err "unsupported architecture: $uname_m" ;;
esac
PLATFORM="${os}-${arch}"
info "platform: ${PLATFORM}"

# ---- http helper ----
http_get() {
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL --retry 3 "$@"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO- "$@"
  else
    err "need either curl or wget"
  fi
}
http_download() {
  local url="$1" dest="$2"
  if command -v curl >/dev/null 2>&1; then
    curl -fSL --retry 3 -o "$dest" "$url" \
      || err "download failed: $url"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO "$dest" "$url" \
      || err "download failed: $url"
  else
    err "need either curl or wget"
  fi
}

# ---- resolve version ----
if [ -z "$VERSION" ]; then
  VERSION="$(http_get "https://api.github.com/repos/${REPO}/releases/latest" \
    | grep '"tag_name"' | sed 's/.*"tag_name": *"\(.*\)".*/\1/')"
  [ -n "$VERSION" ] || err "could not resolve latest version from GitHub API"
fi
info "version: ${VERSION}"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# ---- download sha256sums ----
SUMS_URL="https://github.com/${REPO}/releases/download/${VERSION}/sha256sums.txt"
http_download "$SUMS_URL" "$TMP/sha256sums.txt"

# ---- download + verify each binary tarball ----
install_tarball() {
  local asset="$1"
  local tarball="${asset}.tar.gz"
  local url="https://github.com/${REPO}/releases/download/${VERSION}/${tarball}"

  info "downloading ${tarball}"
  http_download "$url" "$TMP/$tarball"

  # Verify checksum
  expected="$(grep " ${tarball}$" "$TMP/sha256sums.txt" | awk '{print $1}')"
  [ -n "$expected" ] || err "${tarball} not found in sha256sums.txt"
  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$TMP/$tarball" | awk '{print $1}')"
  elif command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "$TMP/$tarball" | awk '{print $1}')"
  else
    err "neither sha256sum nor shasum is available"
  fi
  [ "$actual" = "$expected" ] || \
    err "checksum mismatch for ${tarball}\n  expected: $expected\n  got: $actual"
  ok "checksum verified"

  tar -xzf "$TMP/$tarball" -C "$TMP"
}

install_tarball "kotlin-lsp-${PLATFORM}"
install_tarball "kotlin-jar-indexer-${PLATFORM}"

# ---- pick install prefix ----
mkdir -p "$PREFIX" 2>/dev/null || true
SUDO=""
if [ ! -w "$PREFIX" ]; then
  if [ -w /usr/local/bin ]; then
    PREFIX="/usr/local/bin"
  elif command -v sudo >/dev/null 2>&1; then
    info "elevating to write to /usr/local/bin"
    SUDO="sudo"
    PREFIX="/usr/local/bin"
  else
    err "no writable install prefix; set KOTLIN_LSP_PREFIX or rerun with sudo"
  fi
fi

# ---- install binaries ----
for bin in kotlin-lsp kotlin-jar-indexer; do
  src="$TMP/$bin"
  [ -f "$src" ] || err "expected binary '$bin' not found in archive"
  chmod +x "$src"
  ${SUDO} install -m 0755 "$src" "$PREFIX/$bin"
  ok "${bin} → ${PREFIX}/${bin}"
done

# ---- verify ----
"$PREFIX/kotlin-lsp" --version >/dev/null 2>&1 \
  || err "binary at $PREFIX/kotlin-lsp did not run — try \`$PREFIX/kotlin-lsp --version\`"
info "$("$PREFIX/kotlin-lsp" --version)"

# ---- PATH hint ----
case ":${PATH:-}:" in
  *":$PREFIX:"*) ;;
  *)
    printf '\n\033[33m!\033[0m %s is not in your PATH. Add it:\n' "$PREFIX"
    printf '    echo '\''export PATH="%s:$PATH"'\'' >> ~/.zshrc\n' "$PREFIX"
    printf '    echo '\''export PATH="%s:$PATH"'\'' >> ~/.bashrc\n\n' "$PREFIX"
    ;;
esac

printf '\nNext: wire up your editor — see docs at\n  https://github.com/%s#quick-start\n\n' "$REPO"

