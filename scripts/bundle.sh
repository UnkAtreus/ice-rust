#!/usr/bin/env bash
# Builds target/IceRust.app: a release build of ice-rust wrapped in an app
# bundle with LSUIElement (no Dock icon), a stable bundle identifier and an
# ad-hoc signature, so TCC grants (Screen Recording, Accessibility) attach to
# the bundle instead of the terminal that launched it.
#
# This script only builds. It does not launch the app.
#
#   scripts/bundle.sh              build target/IceRust.app
#   BUNDLE_ID=x.y.z scripts/bundle.sh
#   OUT_DIR=dist scripts/bundle.sh build dist/IceRust.app, for example while
#                                  target/IceRust.app is running
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

bundle_id="${BUNDLE_ID:-local.icerust.IceRust}"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
out_dir="${OUT_DIR:-$root/target}"
mkdir -p "$out_dir"
app="$(cd "$out_dir" && pwd)/IceRust.app"

cargo build --release

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$root/target/release/ice-rust" "$app/Contents/MacOS/ice-rust"
cp "$root/assets/AppIcon.icns" "$app/Contents/Resources/AppIcon.icns"
# GPL-3.0: the license and the port notice travel with the binary.
cp "$root/LICENSE" "$root/NOTICE" "$app/Contents/Resources/"

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>ice-rust</string>
    <key>CFBundleIdentifier</key>
    <string>${bundle_id}</string>
    <key>CFBundleName</key>
    <string>IceRust</string>
    <key>CFBundleDisplayName</key>
    <string>IceRust</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>${version}</string>
    <key>CFBundleVersion</key>
    <string>${version}</string>
    <key>LSMinimumSystemVersion</key>
    <string>14.0</string>
    <key>LSUIElement</key>
    <true/>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
PLIST

plutil -lint "$app/Contents/Info.plist" >/dev/null
codesign --force --sign - --identifier "$bundle_id" "$app"
codesign --verify --verbose=1 "$app"

echo "built $app ($bundle_id, version $version)"
echo "launch it yourself with: open \"$app\""
