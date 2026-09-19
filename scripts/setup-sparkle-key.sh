#!/bin/sh
# Creates the Sparkle update signing key, or reuses the one already in the
# login keychain, then stores the private key as the SPARKLE_PRIVATE_KEY GitHub
# secret and writes the public key into crates/diktafon/resources/Info.plist.
#
# The exported private key is the only copy outside this Mac's keychain. Keep
# it somewhere safe: every installed diktafon trusts only updates signed with
# it, so losing the key means users have to reinstall by hand.
set -eu

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
cd "$script_dir/.."

plist=crates/diktafon/resources/Info.plist

gh auth status >/dev/null 2>&1 || { echo "Run 'gh auth login' first." >&2; exit 1; }
repo=$(gh repo view --json nameWithOwner -q .nameWithOwner)
tools="$(./scripts/fetch-sparkle.sh)/bin"

# Every installed copy trusts only the key whose public half is in the
# plist. Generating a fresh one on another Mac would rotate it silently and
# orphan them all, so an existing, different key stops here.
committed=$(/usr/libexec/PlistBuddy -c "Print :SUPublicEDKey" "$plist" 2>/dev/null || true)
"$tools/generate_keys" >/dev/null
public_key=$("$tools/generate_keys" -p)
test -n "$public_key"
if [ -n "$committed" ] && [ "$committed" != "$public_key" ] && [ "${1:-}" != "--rotate" ]; then
  echo "This Mac's Sparkle key differs from the one in $plist." >&2
  echo "Import the original key into this keychain, or pass --rotate to replace it and orphan every installed copy." >&2
  exit 1
fi

printf 'Path to write the private key backup (outside the repository): '
read -r backup
case "$backup" in
  "~") backup="$HOME" ;;
  "~/"*) backup="$HOME/${backup#"~/"}" ;;
  /*) ;;
  *) backup="$PWD/$backup" ;;
esac
case "$backup" in
  "$PWD" | "$PWD"/* | "") echo "Choose a path outside the repository." >&2; exit 1 ;;
esac
mkdir -p "$(dirname "$backup")"

"$tools/generate_keys" -x "$backup"

gh secret set SPARKLE_PRIVATE_KEY --repo "$repo" <"$backup"
/usr/libexec/PlistBuddy -c "Delete :SUPublicEDKey" "$plist" 2>/dev/null || true
/usr/libexec/PlistBuddy -c "Add :SUPublicEDKey string $public_key" "$plist"

echo "Private key backed up to $backup and stored as SPARKLE_PRIVATE_KEY on $repo."
echo "Public key written to $plist. Commit that change."
