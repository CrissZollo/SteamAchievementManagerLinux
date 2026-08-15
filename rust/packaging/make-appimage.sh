#!/bin/bash
# Builds sam-<version>-x86_64.AppImage from an existing release build.
#
# What this does and does not achieve
# -----------------------------------
# An AppImage does NOT bundle glibc, so it does not widen the range of
# distributions that can run the binary. That is decided entirely by the glibc
# the binary was compiled against — see the release workflow, which builds in
# an ubuntu:22.04 container for a 2.35 floor. Build this on a rolling-release
# desktop and you get a single-file bundle that still only runs on rolling
# releases.
#
# It also bundles no libraries, because there is nothing to bundle: sam links
# only libc, libm and libgcc_s. X11, Wayland and GL are dlopened at run time
# and deliberately left to the host, since shipping our own libGL is a
# reliable way to break drivers.
#
# What it does buy is a single file the user can chmod +x and run, with
# automatic desktop integration under AppImageLauncher.
set -euo pipefail

here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
root=$(cd -- "$here/.." && pwd)
cd "$root"

version=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
app_id="io.github.crisszollo.SteamAchievementManager"
binary="target/release/sam"
appdir="dist/sam.AppDir"
output="dist/sam-${version}-x86_64.AppImage"
tools="${APPIMAGE_TOOL_DIR:-dist/.tools}"

if [ ! -x "$binary" ]; then
    echo "make-appimage: $binary not found; run 'make build' first" >&2
    exit 1
fi

mkdir -p "$tools"

# appimagetool is itself an AppImage, so it would normally want FUSE. In a
# container or on a host without libfuse2 that fails, and extracting instead
# works everywhere.
export APPIMAGE_EXTRACT_AND_RUN=1

appimagetool="$tools/appimagetool"
if [ ! -x "$appimagetool" ]; then
    echo "==> fetching appimagetool"
    curl -fsSL -o "$appimagetool" \
        "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage"
    chmod +x "$appimagetool"
fi

# The default type-2 runtime requires libfuse2 on the *user's* machine, which
# Ubuntu 22.04 and later no longer install by default. The static runtime
# removes that requirement, so prefer it and fall back if it cannot be had.
runtime="$tools/runtime-x86_64"
runtime_args=()
if [ ! -f "$runtime" ]; then
    echo "==> fetching static AppImage runtime"
    if ! curl -fsSL -o "$runtime" \
        "https://github.com/AppImage/type2-runtime/releases/download/continuous/runtime-x86_64"; then
        echo "    could not fetch it; falling back to appimagetool's default runtime"
        echo "    (users may then need libfuse2 installed)"
        rm -f "$runtime"
    fi
fi
if [ -f "$runtime" ]; then
    runtime_args=(--runtime-file "$runtime")
fi

echo "==> assembling AppDir"
rm -rf "$appdir"
mkdir -p "$appdir/usr/bin" "$appdir/usr/share/applications"

install -m755 "$binary" "$appdir/usr/bin/sam"
install -m644 "packaging/$app_id.desktop" "$appdir/usr/share/applications/$app_id.desktop"
install -m644 "packaging/$app_id.desktop" "$appdir/$app_id.desktop"

# AppStream metadata, for software centres and AppImageHub.
#
# appimagetool derives the expected name from the desktop file's basename and
# looks for the older `.appdata.xml` spelling. Because the desktop file is now
# named after the component id, that also satisfies AppStream's rule that the
# metainfo filename match the id. It insists on validating
# the file with appstreamcli and fails outright when that is missing, so only
# include the metadata when the validator is present. Leaving it out costs
# nothing but a warning; CI installs `appstream` so releases carry it.
if command -v appstreamcli >/dev/null 2>&1; then
    mkdir -p "$appdir/usr/share/metainfo"
    install -m644 "packaging/$app_id.metainfo.xml" \
        "$appdir/usr/share/metainfo/$app_id.appdata.xml"
else
    echo "    appstreamcli not found; building without AppStream metadata"
fi

for size in 16 32 48; do
    case $size in
        48) src="packaging/sam.png" ;;
        *)  src="packaging/sam-$size.png" ;;
    esac
    [ -f "$src" ] || continue
    mkdir -p "$appdir/usr/share/icons/hicolor/${size}x${size}/apps"
    install -m644 "$src" "$appdir/usr/share/icons/hicolor/${size}x${size}/apps/sam.png"
done
# appimagetool wants an icon at the AppDir root matching the desktop Icon= key.
install -m644 packaging/sam.png "$appdir/sam.png"

# AppRun resolves its own location so the bundle is relocatable.
cat > "$appdir/AppRun" <<'EOF'
#!/bin/sh
HERE=$(dirname "$(readlink -f "$0")")
export PATH="$HERE/usr/bin:$PATH"
exec "$HERE/usr/bin/sam" "$@"
EOF
chmod +x "$appdir/AppRun"

echo "==> building $output"
rm -f "$output"
"$appimagetool" "${runtime_args[@]}" "$appdir" "$output" >/dev/null
chmod +x "$output"

sha256sum "$output" > "$output.sha256"

echo
echo "Built $output"
echo "Minimum glibc for this AppImage:"
objdump -T "$binary" | grep -oE 'GLIBC_[0-9]+\.[0-9]+(\.[0-9]+)?' \
    | sort -uV | tail -1 | sed 's/^/  /'
