#!/usr/bin/env bash
# Regenerates the committed app icons from hydra.svg (macOS only: uses AppKit via JXA,
# iconutil and the system python3):
#   packaging/macos/Hydra.icns   packaging/windows/hydra.ico
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
pkg="$(dirname "$here")"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# render SIZE OUT.png: draw the SVG straight at each size so small icons stay crisp.
render() {
  osascript -l JavaScript - "$here/hydra.svg" "$1" "$2" <<'JS'
ObjC.import('AppKit');
function run(argv) {
  const svg = argv[0], size = parseInt(argv[1]), out = argv[2];
  const img = $.NSImage.alloc.initWithContentsOfFile(svg);
  if (img.isNil()) throw new Error('cannot load ' + svg);
  const rep = $.NSBitmapImageRep.alloc.initWithBitmapDataPlanesPixelsWidePixelsHighBitsPerSampleSamplesPerPixelHasAlphaIsPlanarColorSpaceNameBytesPerRowBitsPerPixel(
    null, size, size, 8, 4, true, false, $.NSDeviceRGBColorSpace, 0, 0);
  $.NSGraphicsContext.saveGraphicsState;
  $.NSGraphicsContext.setCurrentContext($.NSGraphicsContext.graphicsContextWithBitmapImageRep(rep));
  img.drawInRectFromRectOperationFraction($.NSMakeRect(0, 0, size, size), $.NSZeroRect, $.NSCompositingOperationSourceOver, 1.0);
  $.NSGraphicsContext.restoreGraphicsState;
  if (!rep.representationUsingTypeProperties($.NSBitmapImageFileTypePNG, $()).writeToFileAtomically(out, true))
    throw new Error('cannot write ' + out);
}
JS
}

iconset="$work/Hydra.iconset"
mkdir "$iconset"
for s in 16 32 128 256 512; do
  render "$s" "$iconset/icon_${s}x${s}.png"
  render "$((s * 2))" "$iconset/icon_${s}x${s}@2x.png"
done
iconutil -c icns -o "$pkg/macos/Hydra.icns" "$iconset"

# Windows: an ICO whose entries are PNGs (supported since Vista).
for s in 16 24 32 48 64 256; do render "$s" "$work/ico_$s.png"; done
python3 -I - "$pkg/windows/hydra.ico" "$work"/ico_{16,24,32,48,64,256}.png <<'PY'
import struct, sys
out, pngs = sys.argv[1], [open(p, "rb").read() for p in sys.argv[2:]]
sizes = [struct.unpack(">I", png[16:20])[0] for png in pngs]  # IHDR width
head = struct.pack("<HHH", 0, 1, len(pngs))
offset = len(head) + 16 * len(pngs)
entries = b""
for size, png in zip(sizes, pngs):
    entries += struct.pack("<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(png), offset)
    offset += len(png)
with open(out, "wb") as f:
    f.write(head + entries + b"".join(pngs))
PY
echo "wrote $pkg/macos/Hydra.icns and $pkg/windows/hydra.ico"
