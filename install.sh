#!/bin/sh
# Fila installer — downloads pre-built binaries from GitHub Releases.
# Usage: curl -fsSL https://get.fila.dev | bash
#    or: curl -fsSL https://get.fila.dev | bash -s -- --version v0.1.0

set -eu

REPO="faiscadev/fila"
INSTALL_DIR="/usr/local/bin"
VERSION=""

# Parse arguments
while [ $# -gt 0 ]; do
  case "$1" in
    --version) VERSION="$2"; shift 2 ;;
    --install-dir) INSTALL_DIR="$2"; shift 2 ;;
    *) echo "Unknown option: $1"; exit 1 ;;
  esac
done

# Detect OS
OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
case "$OS" in
  linux)  OS="linux" ;;
  darwin) OS="darwin" ;;
  *) echo "Unsupported OS: $OS"; exit 1 ;;
esac

# Detect architecture
ARCH="$(uname -m)"
case "$ARCH" in
  x86_64|amd64)   ARCH="amd64" ;;
  aarch64|arm64)   ARCH="arm64" ;;
  *) echo "Unsupported architecture: $ARCH"; exit 1 ;;
esac

PLATFORM="${OS}-${ARCH}"
echo "Detected platform: ${PLATFORM}"

# Determine version to download
if [ -z "$VERSION" ]; then
  echo "Fetching latest release..."
  VERSION="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name"' | sed -E 's/.*"tag_name": *"([^"]+)".*/\1/')"
  if [ -z "$VERSION" ]; then
    echo "Error: could not determine latest version. Use --version to specify."
    exit 1
  fi
fi

echo "Installing fila ${VERSION} for ${PLATFORM}..."

# Construct download URLs
TARBALL="fila-${VERSION}-${PLATFORM}.tar.gz"
CHECKSUM="${TARBALL}.sha256"
BASE_URL="https://github.com/${REPO}/releases/download/${VERSION}"

# Create temp directory
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

# Download tarball and checksum
echo "Downloading ${TARBALL}..."
curl -fsSL "${BASE_URL}/${TARBALL}" -o "${TMP_DIR}/${TARBALL}"
curl -fsSL "${BASE_URL}/${CHECKSUM}" -o "${TMP_DIR}/${CHECKSUM}"

# Verify checksum
echo "Verifying checksum..."
cd "$TMP_DIR"
if command -v sha256sum >/dev/null 2>&1; then
  sha256sum -c "$CHECKSUM"
elif command -v shasum >/dev/null 2>&1; then
  shasum -a 256 -c "$CHECKSUM"
else
  echo "Warning: no sha256sum or shasum found, skipping checksum verification"
fi

# Extract
tar xzf "$TARBALL"
DIR="$(basename "$TARBALL" .tar.gz)"

# Install
if [ -w "$INSTALL_DIR" ]; then
  cp "${DIR}/fila-server" "${INSTALL_DIR}/"
  cp "${DIR}/fila" "${INSTALL_DIR}/"
  chmod +x "${INSTALL_DIR}/fila-server" "${INSTALL_DIR}/fila"
else
  echo "Installing to ${INSTALL_DIR} (requires sudo)..."
  sudo cp "${DIR}/fila-server" "${INSTALL_DIR}/"
  sudo cp "${DIR}/fila" "${INSTALL_DIR}/"
  sudo chmod +x "${INSTALL_DIR}/fila-server" "${INSTALL_DIR}/fila"
fi

echo ""
echo "Fila ${VERSION} installed successfully!"
echo "  fila-server: ${INSTALL_DIR}/fila-server"
echo "  fila (CLI):  ${INSTALL_DIR}/fila"
echo ""
echo "Start the broker:  fila-server"
echo "CLI help:          fila --help"
