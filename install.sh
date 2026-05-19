#!/usr/bin/env bash
# Portex CLI installer.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/Khasan712/portex-v2/main/install.sh | sh
# or:
#   curl -fsSL https://raw.githubusercontent.com/Khasan712/portex-v2/main/install.sh | sh -s -- v0.2.0

set -e

REPO="Khasan712/portex-v2"
VERSION="${1:-latest}"
INSTALL_DIR="${PORTEX_INSTALL_DIR:-/usr/local/bin}"

# Detect platform.
OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS-$ARCH" in
  Darwin-arm64)        ASSET="portex-darwin-arm64" ;;
  Darwin-x86_64)       ASSET="portex-darwin-amd64" ;;
  Linux-x86_64)        ASSET="portex-linux-amd64" ;;
  Linux-aarch64)       ASSET="portex-linux-arm64" ;;
  MINGW*-x86_64 | MSYS*-x86_64 | CYGWIN*-x86_64) ASSET="portex-windows-amd64.exe" ;;
  *)
    echo "Unsupported platform: $OS-$ARCH" >&2
    echo "Currently published: darwin-arm64, darwin-amd64, linux-amd64, linux-arm64, windows-amd64" >&2
    exit 1
    ;;
esac

if [ "$VERSION" = "latest" ]; then
  URL="https://github.com/$REPO/releases/latest/download/$ASSET"
else
  URL="https://github.com/$REPO/releases/download/$VERSION/$ASSET"
fi

echo "Downloading $ASSET ($VERSION) from $URL"

TMP="$(mktemp)"
if ! curl -fsSL "$URL" -o "$TMP"; then
  echo "Download failed. Check that the release '$VERSION' has asset '$ASSET'." >&2
  rm -f "$TMP"
  exit 1
fi

# Install with sudo only if we can't write the dir ourselves.
if [ -w "$INSTALL_DIR" ]; then
  install -m 755 "$TMP" "$INSTALL_DIR/portex"
else
  sudo install -m 755 "$TMP" "$INSTALL_DIR/portex"
fi
rm -f "$TMP"

echo "✓ portex installed to $INSTALL_DIR/portex"
"$INSTALL_DIR/portex" --version 2>/dev/null || true

cat <<EOF

Next steps:
  1. Get an auth token from your dashboard (https://portex.live/dashboard/).
  2. Save it:  portex auth <TOKEN>
  3. Tunnel a local port:  portex http -s <subdomain> -p 3000

Docs: https://github.com/$REPO
EOF
