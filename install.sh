#!/usr/bin/env bash
# lzsync one-liner installer.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/FanFusion/lzsync/main/install.sh | bash
#
# Env overrides:
#   INSTALL_DIR  destination directory (default: $HOME/.local/bin)
#   VERSION      specific version tag, e.g. VERSION=v0.5.0 (default: latest)
#
# v0.5.0+: release tarballs now ship a bundled `rsync` alongside `lzsync`,
# installed as `lzsync-rsync` so it doesn't shadow the system rsync. lzsync
# will automatically prefer the bundled binary when it's a sibling on disk.

set -euo pipefail

REPO="FanFusion/lzsync"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${VERSION:-latest}"

red()    { printf '\033[0;31m%s\033[0m\n' "$*"; }
green()  { printf '\033[0;32m%s\033[0m\n' "$*"; }
yellow() { printf '\033[0;33m%s\033[0m\n' "$*"; }
bold()   { printf '\033[1m%s\033[0m\n' "$*"; }

bold "lzsync installer"
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

artifact="lzsync-${os}-${arch}"
echo "Platform: ${os}/${arch} -> ${artifact}"

# --- Pick download URL ------------------------------------------------------

if [ "$VERSION" = "latest" ]; then
    url="https://github.com/${REPO}/releases/latest/download/${artifact}.tar.gz"
else
    url="https://github.com/${REPO}/releases/download/${VERSION}/${artifact}.tar.gz"
fi

echo "Source:   $url"
echo "Install:  $INSTALL_DIR/lzsync   +   $INSTALL_DIR/lzsync-rsync"
echo

# --- Prep destination -------------------------------------------------------

mkdir -p "$INSTALL_DIR"

# --- Download and extract --------------------------------------------------

tmpdir=$(mktemp -d -t lzsync.XXXXXX)
trap 'rm -rf "$tmpdir"' EXIT
archive="$tmpdir/pkg.tar.gz"

if command -v curl >/dev/null 2>&1; then
    if ! curl -fL --progress-bar -o "$archive" "$url"; then
        red "Download failed: $url"
        exit 1
    fi
elif command -v wget >/dev/null 2>&1; then
    if ! wget -q --show-progress -O "$archive" "$url"; then
        red "Download failed: $url"
        exit 1
    fi
else
    red "Neither curl nor wget is available."
    exit 1
fi

if ! tar -xzf "$archive" -C "$tmpdir"; then
    red "Extract failed from $archive"
    exit 1
fi

# Older releases shipped a raw binary at the artifact name (no tarball).
# Handle both layouts so an INSTALL_DIR wired to a pre-0.5.0 VERSION still
# works.
extracted_dir="$tmpdir/$artifact"
if [ -d "$extracted_dir" ]; then
    src_lzsync="$extracted_dir/lzsync"
    src_rsync="$extracted_dir/lzsync-rsync"
else
    red "Unexpected archive layout; looked for $extracted_dir"
    exit 1
fi

# --- Install ----------------------------------------------------------------

install -m 755 "$src_lzsync" "$INSTALL_DIR/lzsync"
if [ -f "$src_rsync" ]; then
    install -m 755 "$src_rsync" "$INSTALL_DIR/lzsync-rsync"
fi

installed_version=$("$INSTALL_DIR/lzsync" --version 2>/dev/null || echo "lzsync")
green "Installed $installed_version -> $INSTALL_DIR/lzsync"
if [ -f "$INSTALL_DIR/lzsync-rsync" ]; then
    rsync_ver=$("$INSTALL_DIR/lzsync-rsync" --version 2>/dev/null | head -n 1 || echo "lzsync-rsync")
    green "Bundled $rsync_ver -> $INSTALL_DIR/lzsync-rsync"
fi

# --- macOS gatekeeper hint --------------------------------------------------

if [ "$os" = "macos" ]; then
    for bin in "$INSTALL_DIR/lzsync" "$INSTALL_DIR/lzsync-rsync"; do
        [ -f "$bin" ] || continue
        if xattr "$bin" 2>/dev/null | grep -q com.apple.quarantine; then
            yellow "macOS marked $bin as quarantined. Removing…"
            xattr -d com.apple.quarantine "$bin" || true
        fi
    done
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
green "Done. Try: lzsync --help"
