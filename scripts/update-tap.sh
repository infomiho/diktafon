#!/bin/sh
# Points the diktafon cask in infomiho/homebrew-tap at the given tag's DMG:
# rewrites version and sha256 from the packaged image's checksum file, then
# commits and pushes over SSH with the tap's deploy key from TAP_DEPLOY_KEY.
# A cask that already matches is left alone.
set -eu

tag=${1:?usage: update-tap.sh <tag> [archives-dir]}
archives=${2:-dist}
: "${TAP_DEPLOY_KEY:?}"

# GitHub's published SSH host key (https://docs.github.com/en/authentication/
# keychain-and-git/githubs-ssh-key-fingerprints), pinned so the push never
# trusts whatever answers on first use.
github_host_key="github.com ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl"

version=${tag#v}
set -- "$archives"/diktafon-"$version"-macOS-*.dmg.sha256
test "$#" -eq 1 && test -f "$1" || { echo "expected exactly one checksum file for $version in $archives" >&2; exit 1; }
checksum=$(cat "$1")
sha256=${checksum%% *}
test "${#sha256}" -eq 64

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
key="$work/deploy_key"
printf '%s\n' "$TAP_DEPLOY_KEY" >"$key"
chmod 600 "$key"
printf '%s\n' "$github_host_key" >"$work/known_hosts"
export GIT_SSH_COMMAND="ssh -i $key -o IdentitiesOnly=yes -o UserKnownHostsFile=$work/known_hosts -o StrictHostKeyChecking=yes"

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
if git diff --quiet -- Casks/diktafon.rb; then
  echo "diktafon cask already at $version"
  exit 0
fi
git config user.name "github-actions[bot]"
git config user.email "41898282+github-actions[bot]@users.noreply.github.com"
git add Casks/diktafon.rb
git commit -q -m "diktafon $version"
git push -q origin HEAD
echo "diktafon cask updated to $version"
