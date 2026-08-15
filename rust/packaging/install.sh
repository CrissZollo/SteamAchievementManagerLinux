#!/bin/sh
# Installs sam for the current user. No root required.
set -eu

APP_ID="io.github.crisszollo.SteamAchievementManager"
PREFIX="${PREFIX:-$HOME/.local}"
BINDIR="$PREFIX/bin"
APPDIR="$PREFIX/share/applications"
ICONDIR="$PREFIX/share/icons/hicolor"

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

if [ ! -f "$here/sam" ]; then
    echo "install.sh: sam binary not found next to this script" >&2
    exit 1
fi

mkdir -p "$BINDIR" "$APPDIR"
install -m755 "$here/sam" "$BINDIR/sam"
install -m644 "$here/$APP_ID.desktop" "$APPDIR/$APP_ID.desktop"

# Icons are shipped at their native sizes. The source artwork only goes up to
# 48x48, and an upscaled 256 looks worse than letting the toolkit scale.
for size in 16 32 48; do
    case $size in
        48) src="$here/sam.png" ;;
        *)  src="$here/sam-$size.png" ;;
    esac
    [ -f "$src" ] || continue
    mkdir -p "$ICONDIR/${size}x${size}/apps"
    install -m644 "$src" "$ICONDIR/${size}x${size}/apps/sam.png"
done

# Best effort; the entry still works without a cache refresh.
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -q -t -f "$ICONDIR" 2>/dev/null || true
fi
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database -q "$APPDIR" 2>/dev/null || true
fi

echo "Installed $BINDIR/sam"

case ":$PATH:" in
    *":$BINDIR:"*) ;;
    *)
        echo
        echo "Warning: $BINDIR is not on your PATH."
        echo "Add this to your shell profile:"
        echo "    export PATH=\"\$PATH:$BINDIR\""
        ;;
esac

echo
echo "Start Steam, sign in, then run: sam"
