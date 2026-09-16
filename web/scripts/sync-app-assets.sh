#!/bin/sh
# Copies the shared app artwork into web/static so the site builds from web/
# alone. Run after changing assets/diktafon-mark.svg or assets/diktafon.webp.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd)

cp -f "$root/assets/diktafon-mark.svg" "$root/web/static/diktafon-mark.svg"
cp -f "$root/assets/diktafon.webp" "$root/web/static/diktafon.webp"

echo "synced app assets into web/static"
