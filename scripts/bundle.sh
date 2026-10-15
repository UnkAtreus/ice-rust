#!/usr/bin/env bash
# Builds target/IceRust.app: a release build of ice-rust wrapped in an app
# bundle with LSUIElement (no Dock icon) and a stable bundle identifier, so
# TCC grants (Screen Recording, Accessibility) attach to the bundle instead
# of the terminal that launched it.
#
# The signature decides whether those grants survive a rebuild. TCC stores
# the designated requirement of the build that was granted. A build signed
# with the "ice-rust Release Signing" certificate has the requirement
# `identifier "<bundle id>" and certificate leaf = H"<certificate>"`, which
# every later build signed with it meets. An ad-hoc build's requirement is
# its own cdhash, so each rebuild loses the grants.
#
# This script only builds. It does not launch the app.
#
#   scripts/bundle.sh              build target/IceRust.app, signed with the
#                                  release certificate when the keychain has
#                                  it, ad-hoc otherwise
#   SIGN_IDENTITY=- scripts/bundle.sh          ad-hoc signature
#   SIGN_IDENTITY=<SHA-1> scripts/bundle.sh    this identity
#   BUNDLE_ID=x.y.z scripts/bundle.sh
#   OUT_DIR=dist scripts/bundle.sh build dist/IceRust.app, for example while
#                                  target/IceRust.app is running
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

bundle_id="${BUNDLE_ID:-local.icerust.IceRust}"
release_cert="ice-rust Release Signing"
# SHA-1 of the release identity in the keychain search list, or nothing.
# The certificate is self-signed and untrusted, so codesign finds it by its
# hash but not by its name.
release_identity() {
    security find-identity -p codesigning 2>/dev/null |
        awk -v name="\"$release_cert\"" 'index($0, name) { print $2; exit }'
}
identity="${SIGN_IDENTITY:-$(release_identity)}"
identity="${identity:--}"
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
codesign --force --sign "$identity" --identifier "$bundle_id" "$app"
codesign --verify --verbose=1 "$app"
if [[ "$identity" == "-" ]]; then
    signed_with="ad-hoc signature"
else
    signed_with="signed by $identity"
fi

echo "built $app ($bundle_id, version $version, $signed_with)"
echo "launch it yourself with: open \"$app\""
