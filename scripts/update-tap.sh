#!/bin/sh
# Points the diktafon cask in infomiho/homebrew-tap at the given tag's DMG:
# rewrites version and sha256 from the packaged image's checksum file, then
# commits and pushes with TAP_GITHUB_TOKEN.
set -eu

tag=${1:?usage: update-tap.sh <tag> [archives-dir]}
archives=${2:-dist}
: "${TAP_GITHUB_TOKEN:?}"

version=${tag#v}
checksum=$(cat "$archives/diktafon-$version-macOS-arm64.dmg.sha256")
sha256=${checksum%% *}
test "${#sha256}" -eq 64

tap=$(mktemp -d)
trap 'rm -rf "$tap"' EXIT
git clone -q --depth 1 "https://x-access-token:$TAP_GITHUB_TOKEN@github.com/infomiho/homebrew-tap.git" "$tap"
cask="$tap/Casks/diktafon.rb"
sed -i '' \
  -e "s/^  version \".*\"/  version \"$version\"/" \
  -e "s/^  sha256 \".*\"/  sha256 \"$sha256\"/" \
  "$cask"
grep -q "version \"$version\"" "$cask"
grep -q "sha256 \"$sha256\"" "$cask"

cd "$tap"
git config user.name "github-actions[bot]"
git config user.email "41898282+github-actions[bot]@users.noreply.github.com"
git add Casks/diktafon.rb
git commit -q -m "diktafon $version"
git push -q origin HEAD
echo "diktafon cask updated to $version"
