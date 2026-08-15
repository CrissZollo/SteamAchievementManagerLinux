#!/bin/sh
# Installs sam for the current user. No root required.
set -eu

PREFIX="${PREFIX:-$HOME/.local}"
BINDIR="$PREFIX/bin"
APPDIR="$PREFIX/share/applications"

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

if [ ! -f "$here/sam" ]; then
    echo "install.sh: sam binary not found next to this script" >&2
    exit 1
fi

mkdir -p "$BINDIR" "$APPDIR"
install -m755 "$here/sam" "$BINDIR/sam"
install -m644 "$here/sam.desktop" "$APPDIR/sam.desktop"

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
