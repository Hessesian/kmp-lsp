#!/usr/bin/env sh
# install.sh — download and install kotlin-lsp + kotlin-jar-indexer (native sidecar)
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/Hessesian/kotlin-lsp/main/install.sh | sh
#   curl -fsSL https://raw.githubusercontent.com/Hessesian/kotlin-lsp/main/install.sh | sh -s -- --version v0.18.0
#   INSTALL_DIR=/usr/local/bin sh install.sh

set -e

REPO="Hessesian/kotlin-lsp"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.cargo/bin}"

# Parse --version flag
VERSION=""
while [ $# -gt 0 ]; do
  case "$1" in
    --version)
      if [ -z "$2" ]; then
        echo "Error: --version requires a value (e.g. --version v0.18.0)"; exit 1
      fi
      VERSION="$2"; shift 2 ;;
    *) echo "Unknown argument: $1"; exit 1 ;;
  esac
done

# Detect platform
OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
  Linux)  OS_NAME="linux"  ;;
  Darwin) OS_NAME="darwin" ;;
  *)      echo "Unsupported OS: $OS"; exit 1 ;;
esac

case "$ARCH" in
  x86_64 | amd64) ARCH_NAME="x86_64"  ;;
  aarch64 | arm64) ARCH_NAME="aarch64" ;;
  *)               echo "Unsupported architecture: $ARCH"; exit 1 ;;
esac

PLATFORM="${OS_NAME}-${ARCH_NAME}"

# Resolve version
if [ -z "$VERSION" ]; then
  VERSION="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
    | grep '"tag_name"' | sed 's/.*"tag_name": *"\(.*\)".*/\1/')"
fi

echo "Installing kotlin-lsp ${VERSION} for ${PLATFORM} → ${INSTALL_DIR}"

# Download combined tarball
TARBALL="kotlin-lsp-${PLATFORM}.tar.gz"
URL="https://github.com/${REPO}/releases/download/${VERSION}/${TARBALL}"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

echo "Downloading ${URL} ..."
curl -fsSL --progress-bar "$URL" -o "${TMP}/${TARBALL}"

# Verify SHA256 checksum
SUMS_URL="https://github.com/${REPO}/releases/download/${VERSION}/sha256sums.txt"
echo "Verifying checksum ..."
curl -fsSL "$SUMS_URL" -o "${TMP}/sha256sums.txt"
EXPECTED="$(grep " ${TARBALL}$" "${TMP}/sha256sums.txt" | awk '{print $1}')"
if [ -z "$EXPECTED" ]; then
  echo "Error: ${TARBALL} not found in sha256sums.txt"; exit 1
fi
if command -v sha256sum >/dev/null 2>&1; then
  ACTUAL="$(sha256sum "${TMP}/${TARBALL}" | awk '{print $1}')"
elif command -v shasum >/dev/null 2>&1; then
  ACTUAL="$(shasum -a 256 "${TMP}/${TARBALL}" | awk '{print $1}')"
else
  echo "Error: neither sha256sum nor shasum is available"; exit 1
fi
if [ "$ACTUAL" != "$EXPECTED" ]; then
  echo "Error: checksum mismatch for ${TARBALL}"; echo "  expected: $EXPECTED"; echo "  got:      $ACTUAL"; exit 1
fi
echo "  ✓ checksum OK"

# Extract both binaries (named `kotlin-lsp` and `kotlin-jar-indexer` inside the archive)
tar -xzf "${TMP}/${TARBALL}" -C "$TMP"

# Install — fail fast if an expected binary is missing from the archive
mkdir -p "$INSTALL_DIR"
for bin in kotlin-lsp kotlin-jar-indexer; do
  src="${TMP}/${bin}"
  if [ ! -f "$src" ]; then
    echo "Error: expected binary '${bin}' not found in archive"; exit 1
  fi
  chmod +x "$src"
  mv "$src" "${INSTALL_DIR}/${bin}"
  echo "  ✓ ${bin} → ${INSTALL_DIR}/${bin}"
done

# Verify
if command -v kotlin-lsp >/dev/null 2>&1 || [ -x "${INSTALL_DIR}/kotlin-lsp" ]; then
  echo ""
  echo "Installation complete."
  echo "Make sure ${INSTALL_DIR} is on your PATH."
else
  echo ""
  echo "Installation complete. Add ${INSTALL_DIR} to your PATH:"
  echo "  export PATH=\"\$PATH:${INSTALL_DIR}\""
fi
