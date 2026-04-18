#!/usr/bin/env bash
# lzscp one-liner installer.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/FanFusion/lzscp/main/install.sh | bash
#
# Env overrides:
#   INSTALL_DIR  destination directory (default: $HOME/.local/bin)
#   VERSION      specific version tag, e.g. VERSION=v0.1.0 (default: latest)

set -euo pipefail

REPO="FanFusion/lzscp"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${VERSION:-latest}"

red()    { printf '\033[0;31m%s\033[0m\n' "$*"; }
green()  { printf '\033[0;32m%s\033[0m\n' "$*"; }
yellow() { printf '\033[0;33m%s\033[0m\n' "$*"; }
bold()   { printf '\033[1m%s\033[0m\n' "$*"; }

bold "lzscp installer"
echo

# --- Detect platform --------------------------------------------------------

uname_s=$(uname -s)
uname_m=$(uname -m)

case "$uname_s" in
    Linux)  os="linux"  ;;
    Darwin) os="macos"  ;;
    *)      red "Unsupported OS: $uname_s"; exit 1 ;;
esac

case "$uname_m" in
    x86_64|amd64)       arch="x86_64"  ;;
    arm64|aarch64)      arch="aarch64" ;;
    *) red "Unsupported architecture: $uname_m"; exit 1 ;;
esac

artifact="lzscp-${os}-${arch}"
echo "Platform: ${os}/${arch} -> ${artifact}"

# --- Pick download URL ------------------------------------------------------

if [ "$VERSION" = "latest" ]; then
    url="https://github.com/${REPO}/releases/latest/download/${artifact}"
else
    url="https://github.com/${REPO}/releases/download/${VERSION}/${artifact}"
fi

echo "Source:   $url"
echo "Install:  $INSTALL_DIR/lzscp"
echo

# --- Prep destination -------------------------------------------------------

mkdir -p "$INSTALL_DIR"

# --- Download --------------------------------------------------------------

tmp=$(mktemp -t lzscp.XXXXXX)
trap 'rm -f "$tmp"' EXIT

if command -v curl >/dev/null 2>&1; then
    if ! curl -fL --progress-bar -o "$tmp" "$url"; then
        red "Download failed: $url"
        exit 1
    fi
elif command -v wget >/dev/null 2>&1; then
    if ! wget -q --show-progress -O "$tmp" "$url"; then
        red "Download failed: $url"
        exit 1
    fi
else
    red "Neither curl nor wget is available."
    exit 1
fi

# --- Install ----------------------------------------------------------------

install -m 755 "$tmp" "$INSTALL_DIR/lzscp"
installed_version=$("$INSTALL_DIR/lzscp" --version 2>/dev/null || echo "lzscp")
green "Installed $installed_version -> $INSTALL_DIR/lzscp"

# --- macOS gatekeeper hint --------------------------------------------------

if [ "$os" = "macos" ]; then
    if xattr "$INSTALL_DIR/lzscp" 2>/dev/null | grep -q com.apple.quarantine; then
        yellow "macOS marked the binary as quarantined. Removing…"
        xattr -d com.apple.quarantine "$INSTALL_DIR/lzscp" || true
    fi
fi

# --- PATH sanity check ------------------------------------------------------

case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *)
        echo
        yellow "Note: $INSTALL_DIR is not on your PATH."
        echo "Add this to your shell profile (.zshrc / .bashrc):"
        echo "    export PATH=\"$INSTALL_DIR:\$PATH\""
        ;;
esac

echo
green "Done. Try: lzscp --help"
