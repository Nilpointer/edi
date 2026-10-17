#!/usr/bin/env bash
# Build Edi-<version>-x86_64.AppImage into target/appimage/.
# Requires podman (or docker) and curl.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/target/appimage"
TOOLS="$OUT/tools"
APPDIR="$OUT/Edi.AppDir"
VERSION="$(grep -m1 '^version' "$ROOT/Cargo.toml" | cut -d'"' -f2)"
ENGINE="$(command -v podman || command -v docker)"

# Pinned packaging tools.
APPIMAGETOOL_URL="https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-x86_64.AppImage"
APPIMAGETOOL_SHA="ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0"
RUNTIME_URL="https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-x86_64"
RUNTIME_SHA="2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d"

fetch() { # url sha256 dest
    if [ -f "$3" ] && echo "$2  $3" | sha256sum -c --status; then return; fi
    curl -fL --retry 3 -o "$3.part" "$1"
    echo "$2  $3.part" | sha256sum -c --quiet
    mv "$3.part" "$3"
    chmod +x "$3"
}

echo "==> Building edi $VERSION against glibc 2.35"
"$ENGINE" build -q -t edi-builder -f "$ROOT/packaging/Containerfile" "$ROOT/packaging" >/dev/null
mkdir -p "$HOME/.cargo/registry"
"$ENGINE" run --rm --security-opt label=disable \
    -v "$ROOT":/src -v "$HOME/.cargo/registry":/usr/local/cargo/registry \
    -w /src -e CARGO_TARGET_DIR=/src/target/glibc2.35 \
    edi-builder cargo build --release --locked
BIN="$ROOT/target/glibc2.35/release/edi"
NEED="$(objdump -T "$BIN" | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -1)"
echo "    requires $NEED"

echo "==> Assembling AppDir"
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share/applications" "$APPDIR/usr/share/icons/hicolor/scalable/apps"
install -m755 "$BIN" "$APPDIR/usr/bin/edi"
install -m755 "$ROOT/packaging/AppRun" "$APPDIR/AppRun"
install -m644 "$ROOT/packaging/edi.desktop" "$APPDIR/edi.desktop"
install -m644 "$ROOT/packaging/edi.desktop" "$APPDIR/usr/share/applications/edi.desktop"
for size in 16 32 48 64 128 256 512; do
    dir="$APPDIR/usr/share/icons/hicolor/${size}x${size}/apps"
    mkdir -p "$dir"
    install -m644 "$ROOT/assets/icons/edi-$size.png" "$dir/edi.png"
done
install -m644 "$ROOT/assets/edi.svg" "$APPDIR/usr/share/icons/hicolor/scalable/apps/edi.svg"
install -m644 "$ROOT/assets/icons/edi-256.png" "$APPDIR/edi.png"
ln -s edi.png "$APPDIR/.DirIcon"

echo "==> Packing AppImage"
mkdir -p "$TOOLS"
fetch "$APPIMAGETOOL_URL" "$APPIMAGETOOL_SHA" "$TOOLS/appimagetool"
fetch "$RUNTIME_URL" "$RUNTIME_SHA" "$TOOLS/runtime-x86_64"
TARGET="$OUT/Edi-$VERSION-x86_64.AppImage"
ARCH=x86_64 "$TOOLS/appimagetool" --appimage-extract-and-run --no-appstream \
    --runtime-file "$TOOLS/runtime-x86_64" "$APPDIR" "$TARGET"
echo "==> $TARGET"
