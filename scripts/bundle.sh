#!/bin/sh
# Build diktafon.app so macOS attaches microphone and Accessibility permissions
# to the app itself instead of the launching terminal.
#
# Signing identity: a real Apple Development certificate keeps the TCC
# identity stable across rebuilds, so mic and Accessibility grants survive.
# Auto-discovered when exactly one exists; DIKTAFON_CODESIGN_IDENTITY
# overrides. The ad-hoc fallback re-prompts mic on each rebuild and breaks
# Accessibility silently (its checkbox still shows enabled; remove and re-add
# the app in System Settings).
set -eu

cd "$(dirname "$0")/.."

# The Apple Intelligence bridge only compiles against full Xcode; with
# xcode-select pointing at the Command Line Tools the build silently falls
# back to the stub. Point the build at Xcode when it is installed.
if [ -z "${DEVELOPER_DIR:-}" ] && [ -d /Applications/Xcode.app/Contents/Developer ]; then
  export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer
fi

cargo build --release -p diktafon -p diktafond

app="target/diktafon-dev.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
cp crates/diktafon/resources/Info.plist "$app/Contents/Info.plist"
# macOS files a permission against the bundle id AND the code signature. A
# local build is signed with a development certificate, the released app with
# Developer ID, so sharing one bundle id means one of them always reads its
# own grant as denied. Local builds get their own id and name instead.
/usr/libexec/PlistBuddy -c "Set :CFBundleIdentifier com.infomiho.diktafon.dev" "$app/Contents/Info.plist"
# System Settings labels a privacy row from the bundle's name on disk, so the
# bundle is named diktafon-dev.app too; otherwise both builds show as
# "diktafon" and you cannot tell which row is which.
/usr/libexec/PlistBuddy -c "Set :CFBundleName diktafon-dev" "$app/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleDisplayName diktafon-dev" "$app/Contents/Info.plist"
mkdir -p "$app/Contents/Resources"
cp crates/diktafon/resources/diktafon.icns "$app/Contents/Resources/diktafon.icns"
cp THIRD_PARTY_NOTICES.md "$app/Contents/Resources/THIRD_PARTY_NOTICES.md"
cp -R licenses "$app/Contents/Resources/licenses"
cp target/release/diktafon "$app/Contents/MacOS/diktafon"
# Next to the client so its auto-spawn finds it.
cp target/release/diktafond "$app/Contents/MacOS/diktafond"

identity="${DIKTAFON_CODESIGN_IDENTITY:-}"
if [ -z "$identity" ]; then
  identity=$(security find-identity -v -p codesigning | awk -F'"' '/Apple Development/ {print $2; exit}')
fi
if [ -n "$identity" ]; then
  echo "Signing with: $identity"
else
  echo "No Apple Development identity found; signing ad-hoc (permissions reset on rebuild)"
fi
codesign --force --sign "${identity:--}" "$app"
codesign --verify --deep "$app"
plutil -lint "$app/Contents/Info.plist" > /dev/null
test -f "$app/Contents/Resources/THIRD_PARTY_NOTICES.md"
test -f "$app/Contents/Resources/licenses/Apache-2.0.txt"

echo "Built $app"
echo "Launch with: open "$app"  (permissions attach to the app; no console output)"
echo "For console output run it from a terminal instead; permissions then attach to that terminal."
