#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 4 ]; then
    echo "Usage: $0 <x86_64|aarch64> <ms binary> <runtime directory> <output.AppImage>" >&2
    exit 1
fi

export ARCH="$1"
case "$ARCH" in
    x86_64)
        tool_sha=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0
        runtime_sha=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d
        ;;
    aarch64)
        tool_sha=f0837e7448a0c1e4e650a93bb3e85802546e60654ef287576f46c71c126a9158
        runtime_sha=00cbdfcf917cc6c0ff6d3347d59e0ca1f7f45a6df1a428a0d6d8a78664d87444
        ;;
    *) echo "Unsupported AppImage architecture: $ARCH" >&2; exit 1 ;;
esac

repo=$(cd -- "$(dirname -- "$0")/../.." && pwd)
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT
appdir="$work/Mitos.AppDir"
mkdir -p "$appdir/usr/bin"
cp "$2" "$appdir/usr/bin/ms"
chmod 755 "$appdir/usr/bin/ms"
# The loader finds runtime/ beside the executable, including through symlinks.
cp -a "$3" "$appdir/usr/bin/runtime"
rm -rf "$appdir/usr/bin/runtime/grammars/sources"
ln -s usr/bin/ms "$appdir/AppRun"
cp "$repo/contrib/Mitos.desktop" "$appdir/Mitos.desktop"
mkdir -p "$appdir/usr/share/metainfo"
cp "$repo/contrib/Mitos.appdata.xml" "$appdir/usr/share/metainfo/com.github.matoous.Mitos.metainfo.xml"
cp "$repo/book/theme/favicon.svg" "$appdir/Mitos.svg"
cp "$repo/LICENSE" "$appdir/LICENSE"

# Pin both the tool and runtime: appimagetool otherwise downloads the latest runtime.
curl --fail --location --retry 3 \
    "https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-$ARCH.AppImage" \
    -o "$work/appimagetool"
curl --fail --location --retry 3 \
    "https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-$ARCH" \
    -o "$work/runtime"
printf '%s  %s\n' "$tool_sha" "$work/appimagetool" "$runtime_sha" "$work/runtime" | sha256sum -c -
chmod 755 "$work/appimagetool"

# Extraction also works on builders without FUSE or permission to mount images.
APPIMAGE_EXTRACT_AND_RUN=1 "$work/appimagetool" \
    --runtime-file "$work/runtime" "$appdir" "$4"
