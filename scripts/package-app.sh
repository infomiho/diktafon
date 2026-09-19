#!/bin/sh
# Bundles diktafon.app from a built binary and packages it as a DMG in dist/.
#
# With DIKTAFON_CODESIGN_IDENTITY set to a Developer ID Application identity the
# app and the DMG are signed for distribution (hardened runtime, timestamp);
# otherwise they carry an ad-hoc signature for local use.
#
# diktafond ships beside diktafon in Contents/MacOS: the client auto-spawns the
# sibling daemon, so the two travel together in one bundle.
#
# The Sparkle framework that powers in-app updates is embedded from the
# pinned release that fetch-sparkle.sh downloads.
set -eu

profile=${1:-release}
script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
cd "$script_dir/.."

metadata=$(cargo metadata --format-version 1 --no-deps)
target_directory=$(printf '%s\n' "$metadata" | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')
version=$(printf '%s\n' "$metadata" | sed -n 's/.*"name":"diktafon","version":"\([^"]*\)".*/\1/p')
test -n "$target_directory"
test -n "$version"

client="$target_directory/$profile/diktafon"
daemon="$target_directory/$profile/diktafond"
test -x "$client"
test -x "$daemon"

# Sparkle compares CFBundleVersion, so it has to be a monotonic number rather
# than the semver string. Pre-release suffixes have no place in that number,
# which also keeps them out of the single stable update feed.
case "$version" in
  *[!0-9.]* | *..* | .* | *.)
    echo "Version $version must be MAJOR.MINOR.PATCH to package a release." >&2
    exit 1
    ;;
esac
build_number=$(printf '%s\n' "$version" | awk -F. 'NF == 3 { print $1 * 1000000 + $2 * 1000 + $3 }')
test -n "$build_number"

sparkle_framework_source="$(./scripts/fetch-sparkle.sh)/Sparkle.framework"
feed_url="https://github.com/infomiho/diktafon/releases/latest/download/appcast.xml"

identity=${DIKTAFON_CODESIGN_IDENTITY:--}
sign() {
  if [ "$identity" = "-" ]; then
    codesign --force --sign - "$@"
  else
    codesign --force --timestamp --options runtime --sign "$identity" "$@"
  fi
}

# The hardened runtime denies the microphone outright, with no prompt and no
# row in System Settings, unless the signature carries the audio-input
# exception. Only the app itself records, so only it gets the entitlements.
sign_app() {
  if [ "$identity" = "-" ]; then
    codesign --force --sign - "$@"
  else
    codesign --force --timestamp --options runtime \
      --entitlements crates/diktafon/resources/entitlements.plist \
      --sign "$identity" "$@"
  fi
}

app="$target_directory/$profile/diktafon.app"
contents="$app/Contents"
rm -rf "$app"
mkdir -p "$contents/MacOS" "$contents/Resources" dist
cp "$client" "$contents/MacOS/diktafon"
cp "$daemon" "$contents/MacOS/diktafond"
cp crates/diktafon/resources/Info.plist "$contents/Info.plist"
cp crates/diktafon/resources/diktafon.icns "$contents/Resources/diktafon.icns"
cp THIRD_PARTY_NOTICES.md "$contents/Resources/THIRD_PARTY_NOTICES.md"
cp -R licenses "$contents/Resources/licenses"

/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $version" "$contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $build_number" "$contents/Info.plist"
if [ "$identity" != "-" ]; then
  # Only a Developer ID build can be replaced by a release, so only it gets
  # the feed; every other bundle leaves Sparkle inert for lack of one.
  /usr/libexec/PlistBuddy -c "Add :SUFeedURL string $feed_url" "$contents/Info.plist"
fi

# diktafon is not sandboxed, so Sparkle's XPC services never run. They go,
# along with the header and module folders, so the shipped framework carries
# no unsigned nested code and no development artifacts.
sparkle_framework="$contents/Frameworks/Sparkle.framework"
mkdir -p "$contents/Frameworks"
cp -R "$sparkle_framework_source" "$sparkle_framework"
for extra in XPCServices Headers PrivateHeaders Modules; do
  rm -rf "${sparkle_framework:?}/$extra" "${sparkle_framework:?}/Versions/B/$extra"
done

# Copied resources can carry Finder info that codesign rejects as detritus.
xattr -cr "$app"
# Nested code signs first, then the bundle seals over it. Library validation
# under the hardened runtime requires the framework to carry the same
# identity as the app.
sign "$sparkle_framework/Versions/B/Autoupdate"
sign "$sparkle_framework/Versions/B/Updater.app"
sign "$sparkle_framework"
sign "$contents/MacOS/diktafond"
sign_app "$app"
codesign --verify --deep --strict "$app"

architecture=$(uname -m)
image="dist/diktafon-$version-macOS-$architecture.dmg"
staging=$(mktemp -d)
trap 'rm -rf "$staging"' EXIT
cp -R "$app" "$staging/"
ln -s /Applications "$staging/Applications"
rm -f "$image" "$image.sha256"
hdiutil create -quiet -volname "diktafon" -srcfolder "$staging" -ov -format UDZO "$image"
sign "$image"
(cd "$(dirname "$image")" && shasum -a 256 "$(basename "$image")" >"$(basename "$image").sha256")
echo "$image"
