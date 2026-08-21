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

cargo build --release -p diktafon -p diktafond

app=target/diktafon.app
rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
cp crates/diktafon/resources/Info.plist "$app/Contents/Info.plist"
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

echo "Built $app"
echo "Launch with: open $app  (permissions attach to the app; no console output)"
echo "For console output run it from a terminal instead; permissions then attach to that terminal."
