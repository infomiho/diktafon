#!/bin/sh
# Points the diktafon cask in infomiho/homebrew-tap at the given tag's DMG:
# rewrites version and sha256 from the packaged image's checksum file, then
# commits and pushes over SSH with the tap's deploy key from TAP_DEPLOY_KEY.
set -eu

tag=${1:?usage: update-tap.sh <tag> [archives-dir]}
archives=${2:-dist}
: "${TAP_DEPLOY_KEY:?}"

version=${tag#v}
checksum=$(cat "$archives/diktafon-$version-macOS-arm64.dmg.sha256")
sha256=${checksum%% *}
test "${#sha256}" -eq 64

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
key="$work/deploy_key"
printf '%s\n' "$TAP_DEPLOY_KEY" >"$key"
chmod 600 "$key"
ssh-keyscan -t ed25519 github.com >"$work/known_hosts" 2>/dev/null
export GIT_SSH_COMMAND="ssh -i $key -o IdentitiesOnly=yes -o UserKnownHostsFile=$work/known_hosts"

tap="$work/tap"
git clone -q --depth 1 git@github.com:infomiho/homebrew-tap.git "$tap"
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
