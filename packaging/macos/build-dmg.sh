#!/usr/bin/env bash
# Makes "Stream Spectre.app" and a .dmg of it on macOS, from the two programs
# built for each kind of Mac (Intel and Apple Silicon), joined into one that runs
# on both.
#
#   packaging/macos/build-dmg.sh VERSION ARM64_DIR X86_64_DIR OUT_DIR
#
# ARM64_DIR and X86_64_DIR hold stream-spectre-window and stream-spectre as
# `cargo build --release --features window --target ...` leaves them. The app is
# signed ad hoc only: macOS asks the user to allow it the first time (right-click,
# Open), because there is no Apple developer certificate behind it.

set -euo pipefail

version="${1:?version}"
arm64="${2:?directory of the Apple Silicon build}"
x86_64="${3:?directory of the Intel build}"
out="${4:?output directory}"

# macOS wants the numbers of the version in the app (0.1.0), not a suffix (-rc1).
plist_version="${version%%-*}"

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
app="$out/Stream Spectre.app"
rm -rf "$app" "$out/dmg"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$out/dmg"

for name in stream-spectre-window stream-spectre; do
  lipo -create "$arm64/$name" "$x86_64/$name" -output "$app/Contents/MacOS/$name"
  chmod 755 "$app/Contents/MacOS/$name"
done

# The icon: an iconset from the PNG files, made into .icns.
iconset="$out/icon.iconset"
rm -rf "$iconset"
mkdir -p "$iconset"
cp "$repo/assets/icon-16.png"  "$iconset/icon_16x16.png"
cp "$repo/assets/icon-32.png"  "$iconset/icon_16x16@2x.png"
cp "$repo/assets/icon-32.png"  "$iconset/icon_32x32.png"
cp "$repo/assets/icon-64.png"  "$iconset/icon_32x32@2x.png"
cp "$repo/assets/icon-128.png" "$iconset/icon_128x128.png"
cp "$repo/assets/icon-256.png" "$iconset/icon_128x128@2x.png"
cp "$repo/assets/icon-256.png" "$iconset/icon_256x256.png"
cp "$repo/assets/icon-512.png" "$iconset/icon_256x256@2x.png"
cp "$repo/assets/icon-512.png" "$iconset/icon_512x512.png"
iconutil -c icns "$iconset" -o "$app/Contents/Resources/icon.icns"

cat >"$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Stream Spectre</string>
  <key>CFBundleDisplayName</key><string>Stream Spectre</string>
  <key>CFBundleIdentifier</key><string>io.github.fhudint.stream-spectre</string>
  <key>CFBundleVersion</key><string>${plist_version}</string>
  <key>CFBundleShortVersionString</key><string>${plist_version}</string>
  <key>CFBundleExecutable</key><string>stream-spectre-window</string>
  <key>CFBundleIconFile</key><string>icon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

codesign --force --deep --sign - "$app"

cp -R "$app" "$out/dmg/"
ln -s /Applications "$out/dmg/Applications"
hdiutil create -volname "Stream Spectre" -srcfolder "$out/dmg" -ov -format UDZO \
  "$out/stream-spectre-${version}-macos-universal.dmg"
echo "Made $out/stream-spectre-${version}-macos-universal.dmg"
