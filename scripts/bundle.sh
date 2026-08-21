#!/bin/sh
# Build diktafon.app so macOS attaches microphone and Accessibility permissions
# to the app itself instead of the launching terminal.
#
# Ad-hoc signing means the signature (and thus the TCC identity) changes on
# every rebuild: the mic permission re-prompts, but Accessibility fails
# silently while its checkbox still shows enabled; remove and re-add the app in
# System Settings. Good enough until there is a real signing identity.
set -eu

cd "$(dirname "$0")/.."

cargo build --release -p diktafon

app=target/diktafon.app
rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
cp crates/diktafon/resources/Info.plist "$app/Contents/Info.plist"
cp target/release/diktafon "$app/Contents/MacOS/diktafon"

codesign --force --sign - "$app"
codesign --verify --deep "$app"
plutil -lint "$app/Contents/Info.plist" > /dev/null

echo "Built $app"
echo "Launch with: open $app  (permissions attach to the app; no console output)"
echo "For console output run it from a terminal instead; permissions then attach to that terminal."
