#!/bin/sh
# Bundles diktafon.app from a built binary and packages it as a DMG in dist/.
#
# With DIKTAFON_CODESIGN_IDENTITY set to a Developer ID Application identity the
# app and the DMG are signed for distribution (hardened runtime, timestamp);
# otherwise they carry an ad-hoc signature for local use.
#
# diktafond ships beside diktafon in Contents/MacOS: the client auto-spawns the
# sibling daemon, so the two travel together in one bundle.
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

# CFBundleVersion compares as a period-separated number, so keep the version to
# plain digits rather than letting a pre-release suffix leak into the bundle.
case "$version" in
  *[!0-9.]* | *..* | .* | *.)
    echo "Version $version must be MAJOR.MINOR.PATCH to package a release." >&2
    exit 1
    ;;
esac
build_number=$(printf '%s\n' "$version" | awk -F. 'NF == 3 { print $1 * 1000000 + $2 * 1000 + $3 }')
test -n "$build_number"

identity=${DIKTAFON_CODESIGN_IDENTITY:--}
sign() {
  if [ "$identity" = "-" ]; then
    codesign --force --sign - "$@"
  else
    codesign --force --timestamp --options runtime --sign "$identity" "$@"
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

# Copied resources can carry Finder info that codesign rejects as detritus.
xattr -cr "$app"
# The daemon is nested code: it signs first, then the bundle seals over it.
sign "$contents/MacOS/diktafond"
sign "$app"
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
