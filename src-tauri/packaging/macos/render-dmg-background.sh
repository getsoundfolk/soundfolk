#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
#
# Renders dmg-background.svg into dmg-background.tiff, the disk image's background: one TIFF with
# a 72 dpi and a 144 dpi image, as `tiffutil -cathidpicheck` makes them, so that Finder draws it
# sharp on Retina screens too. Run it after changing the SVG, and commit both files.
#
# Needs Google Chrome (or Chromium) and Python with Pillow.
# Usage: render-dmg-background.sh
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
chrome=$(command -v google-chrome || command -v chromium || command -v chromium-browser)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

for scale in 1 2; do
  "$chrome" --headless --disable-gpu --hide-scrollbars --force-device-scale-factor="$scale" \
    --window-size=660,400 --screenshot="$work/$scale.png" "file://$here/dmg-background.svg" \
    2> /dev/null
done

python3 -I - "$work/1.png" "$work/2.png" "$here/dmg-background.tiff" <<'PY'
import sys
from PIL import Image

one, two, out = sys.argv[1:]
images = [Image.open(one).convert("RGB"), Image.open(two).convert("RGB")]
assert images[0].size == (660, 400) and images[1].size == (1320, 800), [i.size for i in images]
# Each page keeps its own resolution: 660 × 400 points both.
images[0].encoderinfo = {"dpi": (72, 72)}
images[1].encoderinfo = {"dpi": (144, 144)}
images[0].save(out, save_all=True, append_images=[images[1]], compression="tiff_lzw", dpi=(72, 72))
PY
echo "wrote $here/dmg-background.tiff"
