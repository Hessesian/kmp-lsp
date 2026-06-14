#!/usr/bin/env bash
# install.sh — download and install kmp-lsp + kmp-jar-indexer (native sidecar)
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/Hessesian/kmp-lsp/main/install.sh | bash
#   curl -fsSL https://raw.githubusercontent.com/Hessesian/kmp-lsp/main/install.sh | bash -s -- --version v0.18.0
#   INSTALL_DIR=/usr/local/bin bash install.sh

set -euo pipefail

REPO="Hessesian/kmp-lsp"

err()  { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }
info() { printf '\033[36m::\033[0m %s\n' "$*"; }
ok()   { printf '\033[32m✓\033[0m %s\n' "$*"; }

# Parse --version flag (also honoured via KMP_LSP_VERSION env var).
VERSION="${KMP_LSP_VERSION:-}"
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

# ---- http helpers ----
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

# ---- resolve install prefix ----
# Priority: KMP_LSP_PREFIX env > INSTALL_DIR env > ~/.cargo/bin if present > ~/.local/bin
PREFIX="${KMP_LSP_PREFIX:-}"
if [ -z "$PREFIX" ]; then
  PREFIX="${INSTALL_DIR:-}"
fi
if [ -z "$PREFIX" ]; then
  if [ -d "$HOME/.cargo/bin" ]; then
    PREFIX="$HOME/.cargo/bin"
  else
    PREFIX="$HOME/.local/bin"
  fi
fi

echo "Installing kmp-lsp ${VERSION} for ${PLATFORM} → ${PREFIX}"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# ---- download sha256sums ----
SUMS_URL="https://github.com/${REPO}/releases/download/${VERSION}/sha256sums.txt"
http_download "$SUMS_URL" "$TMP/sha256sums.txt"

# ---- verify checksum helper ----
verify_checksum() {
  local file="$1" name="$2"
  local expected
  expected="$(awk -v name="${name}" '$2 == name {print $1}' "$TMP/sha256sums.txt")"
  if [ -z "$expected" ]; then
    info "no checksum entry for ${name} — skipping verification"
    return
  fi
  local actual
  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$file" | awk '{print $1}')"
  elif command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "$file" | awk '{print $1}')"
  else
    info "no sha256 tool found — skipping checksum verification"
    return
  fi
  [ "$actual" = "$expected" ] || \
    err "checksum mismatch for ${name}\n  expected: $expected\n  got: $actual"
  ok "checksum verified: ${name}"
}

# ---- download and extract kmp-lsp (tar.gz) ----
LSP_TARBALL="kmp-lsp-${PLATFORM}.tar.gz"
LSP_URL="https://github.com/${REPO}/releases/download/${VERSION}/${LSP_TARBALL}"
info "downloading ${LSP_TARBALL}"
http_download "$LSP_URL" "$TMP/${LSP_TARBALL}"
verify_checksum "$TMP/${LSP_TARBALL}" "${LSP_TARBALL}"
tar -xzf "$TMP/${LSP_TARBALL}" -C "$TMP"
[ -f "$TMP/kmp-lsp" ] || err "kmp-lsp binary not found in ${LSP_TARBALL}"

# ---- download and extract kmp-jar-indexer (plain gz) ----
JAR_GZ="kmp-jar-indexer-${PLATFORM}.gz"
JAR_URL="https://github.com/${REPO}/releases/download/${VERSION}/${JAR_GZ}"
info "downloading ${JAR_GZ}"
http_download "$JAR_URL" "$TMP/${JAR_GZ}"
verify_checksum "$TMP/${JAR_GZ}" "${JAR_GZ}"
gunzip -f "$TMP/${JAR_GZ}"
[ -f "$TMP/kmp-jar-indexer-${PLATFORM}" ] || err "kmp-jar-indexer binary not found after decompression"
mv "$TMP/kmp-jar-indexer-${PLATFORM}" "$TMP/kmp-jar-indexer"

# ---- pick install prefix (elevate if needed) ----
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
    err "no writable install prefix; set KMP_LSP_PREFIX or rerun with sudo"
  fi
fi

# ---- install binaries ----
for bin in kmp-lsp kmp-jar-indexer; do
  src="$TMP/$bin"
  [ -f "$src" ] || err "expected binary '$bin' not found"
  chmod +x "$src"
  ${SUDO} install -m 0755 "$src" "$PREFIX/$bin"
  ok "${bin} → ${PREFIX}/${bin}"
done

echo ""
echo "Installation complete."
if command -v kmp-lsp >/dev/null 2>&1; then
  echo "kmp-lsp is on your PATH."
else
  echo "Make sure ${PREFIX} is on your PATH:"
  echo "  export PATH=\"\$PATH:${PREFIX}\""
fi
