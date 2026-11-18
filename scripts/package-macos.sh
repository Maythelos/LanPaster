#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
ICON_TAG="${VERSION//./}"
APP="$ROOT/dist/LanPaster.app"
ZIP="$ROOT/dist/LanPaster-${VERSION}-macos-universal.zip"

rm -rf "$APP" "$ZIP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$ROOT/dist"

if [[ "${ONLY_ARM64:-0}" == "1" ]]; then
  echo "Building arm64 only..."
  cargo build --release --target aarch64-apple-darwin
  cp "$ROOT/target/aarch64-apple-darwin/release/lanpaster" "$APP/Contents/MacOS/lanpaster"
else
  if ! rustup target list --installed | grep -qx x86_64-apple-darwin; then
    echo "Installing x86_64-apple-darwin target..."
    rustup target add x86_64-apple-darwin
  fi
  echo "Building arm64..."
  cargo build --release --target aarch64-apple-darwin
  echo "Building x86_64..."
  cargo build --release --target x86_64-apple-darwin
  lipo -create \
    "$ROOT/target/aarch64-apple-darwin/release/lanpaster" \
    "$ROOT/target/x86_64-apple-darwin/release/lanpaster" \
    -output "$APP/Contents/MacOS/lanpaster"
fi

chmod +x "$APP/Contents/MacOS/lanpaster"
cp "$ROOT/assets/AppIcon.icns" "$APP/Contents/Resources/AppIcon-${ICON_TAG}.icns"
printf 'APPL????' > "$APP/Contents/PkgInfo"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>zh_CN</string>
    <key>CFBundleDisplayName</key>
    <string>LanPaster</string>
    <key>CFBundleExecutable</key>
    <string>lanpaster</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon-${ICON_TAG}</string>
    <key>CFBundleIdentifier</key>
    <string>com.lanpaster.app</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>LanPaster</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>${VERSION}</string>
    <key>CFBundleVersion</key>
    <string>${VERSION}</string>
    <key>LSMinimumSystemVersion</key>
    <string>13.0</string>
    <key>LSUIElement</key>
    <true/>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSLocalNetworkUsageDescription</key>
    <string>LanPaster 需要访问局域网，用于发现其他设备并同步剪贴板。</string>
    <key>NSBonjourServices</key>
    <array>
        <string>_lanpaster._tcp</string>
    </array>
</dict>
</plist>
PLIST

plutil -lint "$APP/Contents/Info.plist" >/dev/null
codesign --force --deep --sign - "$APP" >/dev/null
codesign --verify --deep --strict "$APP"

# Refresh Finder/IconServices for the local build path. Without this, an old
# generic icon can stick to dist/LanPaster.app even after the .icns changes.
touch "$APP"
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$APP" >/dev/null 2>&1 || true

file "$APP/Contents/MacOS/lanpaster"
echo "App bundle: $APP"
echo "Version:    $VERSION"
echo "Arch:       $(lipo -archs "$APP/Contents/MacOS/lanpaster")"

ditto -c -k --keepParent "$APP" "$ZIP"
echo "Zip:        $ZIP"

DMG="$ROOT/dist/LanPaster-${VERSION}-macos-universal.dmg"
DMG_ROOT="$ROOT/dist/dmg-root"
rm -rf "$DMG_ROOT" "$DMG"
mkdir -p "$DMG_ROOT"
cp -R "$APP" "$DMG_ROOT/"
ln -s /Applications "$DMG_ROOT/Applications"
hdiutil create -volname "LanPaster" -srcfolder "$DMG_ROOT" -ov -format UDZO "$DMG" >/dev/null
rm -rf "$DMG_ROOT"
echo "DMG:        $DMG"
ls -lh "$ZIP" "$DMG"
