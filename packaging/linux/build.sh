#!/usr/bin/env bash
set -euo pipefail

ROOT=/src
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)"
BIN="/build/target/release/lanpaster"
PKG="/tmp/lanpaster-pkg"
DIST="$ROOT/dist"

cd "$ROOT"
echo "Building LanPaster ${VERSION} for linux/amd64..."
cargo build --release

mkdir -p "$DIST" "$PKG/DEBIAN" "$PKG/usr/bin" \
    "$PKG/usr/share/applications" \
    "$PKG/usr/share/icons/hicolor/512x512/apps" \
    "$PKG/usr/share/pixmaps"

cp "$BIN" "$PKG/usr/bin/lanpaster"
chmod 755 "$PKG/usr/bin/lanpaster"
cp "$ROOT/assets/AppIcon-1024.png" "$PKG/usr/share/icons/hicolor/512x512/apps/lanpaster.png"
cp "$ROOT/assets/AppIcon-1024.png" "$PKG/usr/share/pixmaps/lanpaster.png"

cat > "$PKG/usr/share/applications/lanpaster.desktop" <<'DESKTOP'
[Desktop Entry]
Type=Application
Name=LanPaster
Comment=LAN clipboard sync
Exec=lanpaster
Icon=lanpaster
Terminal=false
Categories=Utility;Network;
StartupNotify=false
DESKTOP

# Determine runtime dependencies from the actual binary.
cd "$PKG"
DEPS="$(dpkg-shlibdeps -O usr/bin/lanpaster 2>/dev/null | sed -n 's/^shlibs:Depends=//p' || true)"
if [ -z "$DEPS" ]; then
    DEPS="libc6, libasound2t64 | libasound2, libx11-6, libxcb1, libxkbcommon0, libwayland-client0, libfontconfig1, libfreetype6"
fi

cat > "$PKG/DEBIAN/control" <<CONTROL
Package: lanpaster
Version: ${VERSION}
Section: utils
Priority: optional
Architecture: amd64
Maintainer: LanPaster
Depends: ${DEPS}
Description: LAN clipboard sync for text, images and files
 A tray-based LAN clipboard synchronization tool with mDNS discovery,
 explicit connection approval, and encrypted peer-to-peer transport.
CONTROL

dpkg-deb --build --root-owner-group "$PKG" "$DIST/LanPaster-${VERSION}-linux-amd64.deb"
tar -C "$PKG" -czf "$DIST/LanPaster-${VERSION}-linux-x86_64.tar.gz" usr
cp "$BIN" "$DIST/lanpaster-linux-amd64"

echo "Built:"
ls -lh "$DIST/LanPaster-${VERSION}-linux-amd64.deb" \
       "$DIST/LanPaster-${VERSION}-linux-x86_64.tar.gz" \
       "$DIST/lanpaster-linux-amd64"
echo "Runtime deps: $DEPS"
file "$BIN"
