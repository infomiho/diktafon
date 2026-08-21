#!/bin/sh
# Regenerates crates/diktafon/resources/diktafon.icns from the canonical mark
# geometry: `diktafon --gen-mark` writes assets/AppIcon.svg (and the README
# marks) from crates/diktafon/src/mark.rs, and this script renders the icns
# from it (cadence's pipeline).
set -eu

script_dir=$(CDPATH='' cd -- "$(dirname "$0")" && pwd)
root=$(dirname "$script_dir")
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

cd "$root"
cargo run -q -p diktafon -- --gen-mark

qlmanage -t -s 1024 -o "$work" "$root/assets/AppIcon.svg" >/dev/null
source="$work/AppIcon.svg.png"
iconset="$work/diktafon.iconset"
mkdir -p "$iconset"

render() {
  sips -z "$1" "$1" "$source" --out "$iconset/$2" >/dev/null
}

render 16 icon_16x16.png
render 32 icon_16x16@2x.png
render 32 icon_32x32.png
render 64 icon_32x32@2x.png
render 128 icon_128x128.png
render 256 icon_128x128@2x.png
render 256 icon_256x256.png
render 512 icon_256x256@2x.png
render 512 icon_512x512.png
cp "$source" "$iconset/icon_512x512@2x.png"

iconutil -c icns "$iconset" -o "$root/crates/diktafon/resources/diktafon.icns"
echo "wrote crates/diktafon/resources/diktafon.icns"
