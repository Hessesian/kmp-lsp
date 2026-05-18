#!/usr/bin/env bash
# kotlin-lsp installer for Linux and macOS.
#
# Usage:
#   curl -fsSL https://github.com/qdsfdhvh/kotlin-lsp/releases/latest/download/install.sh | bash
#
# Environment variables:
#   KOTLIN_LSP_VERSION   pin a version (e.g. v0.14.0). Default: latest release.
#   KOTLIN_LSP_REPO      override the source repo (default: qdsfdhvh/kotlin-lsp).
#   KOTLIN_LSP_PREFIX    install directory. Default: $HOME/.local/bin
#                        (falls back to /usr/local/bin if writable and HOME/.local/bin is not on PATH).
set -euo pipefail

REPO="${KOTLIN_LSP_REPO:-qdsfdhvh/kotlin-lsp}"
VERSION="${KOTLIN_LSP_VERSION:-latest}"
PREFIX="${KOTLIN_LSP_PREFIX:-$HOME/.local/bin}"

err() { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }
info() { printf '\033[36m::\033[0m %s\n' "$*"; }

# ---- detect platform ----
uname_s="$(uname -s)"
uname_m="$(uname -m)"
case "$uname_s" in
  Linux)  os="linux" ;;
  Darwin) os="darwin" ;;
  *) err "unsupported OS: $uname_s (this script is for Linux/macOS; use install.ps1 on Windows)" ;;
esac
case "$uname_m" in
  x86_64|amd64) arch="x86_64" ;;
  arm64|aarch64) arch="aarch64" ;;
  *) err "unsupported architecture: $uname_m" ;;
esac
asset="kotlin-lsp-${os}-${arch}"
info "platform: ${os}/${arch} → ${asset}"

# ---- resolve download URL ----
if [ "$VERSION" = "latest" ]; then
  url="https://github.com/${REPO}/releases/latest/download/${asset}.tar.gz"
else
  url="https://github.com/${REPO}/releases/download/${VERSION}/${asset}.tar.gz"
fi
info "downloading ${url}"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

if command -v curl >/dev/null 2>&1; then
  curl -fSL --retry 3 -o "$tmp/asset.tar.gz" "$url" \
    || err "download failed — check that release ${VERSION} exists and includes ${asset}.tar.gz"
elif command -v wget >/dev/null 2>&1; then
  wget -qO "$tmp/asset.tar.gz" "$url" \
    || err "download failed — check that release ${VERSION} exists and includes ${asset}.tar.gz"
else
  err "need either curl or wget"
fi

# ---- extract ----
tar -xzf "$tmp/asset.tar.gz" -C "$tmp"
if [ -f "$tmp/$asset" ]; then
  bin_src="$tmp/$asset"
elif [ -f "$tmp/kotlin-lsp" ]; then
  bin_src="$tmp/kotlin-lsp"
else
  err "tarball did not contain the kotlin-lsp binary (looked for $asset and kotlin-lsp)"
fi
chmod +x "$bin_src"

# ---- pick prefix ----
mkdir -p "$PREFIX" 2>/dev/null || true
if [ ! -w "$PREFIX" ]; then
  # Fall back to /usr/local/bin with sudo if the chosen prefix isn't writable.
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

dest="$PREFIX/kotlin-lsp"
${SUDO:-} install -m 0755 "$bin_src" "$dest"
info "installed → ${dest}"

# ---- verify ----
if ! "$dest" --version >/dev/null 2>&1; then
  err "binary at $dest did not run cleanly — try \`$dest --version\` to debug"
fi
info "$("$dest" --version)"

# ---- PATH hint ----
case ":${PATH:-}:" in
  *":$PREFIX:"*) ;;
  *)
    cat <<EOF

\033[33m!\033[0m $PREFIX is not in your PATH. Add it with one of:

    echo 'export PATH="$PREFIX:\$PATH"' >> ~/.zshrc
    echo 'export PATH="$PREFIX:\$PATH"' >> ~/.bashrc

EOF
    ;;
esac

cat <<'EOF'

Next: wire up your editor — see docs at
  https://github.com/qdsfdhvh/kotlin-lsp#quick-start

EOF
