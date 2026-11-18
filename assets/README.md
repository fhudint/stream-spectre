# Icons

`icon.svg` is the one source: the ghost of the dashboard's logo, in the
dashboard's accent colour, on a dark rounded square. The PNG files are made
from it.

The dashboard serves its own copies from `static/dashboard/` (`favicon.svg`,
`favicon-32.png`, `apple-touch-icon.png`). A test checks that `favicon.svg` is
identical to `icon.svg`; copy it again after a change.

To make the files again (ImageMagick with librsvg):

```sh
for n in 16 32 48 64 128 256 512; do
  magick -background none -density 1536 icon.svg -resize ${n}x${n} PNG32:icon-$n.png
done
# iOS rounds the corners itself, so its icon is a full square:
sed 's/ rx="14"//' icon.svg > /tmp/icon-square.svg
magick -background none -density 1536 /tmp/icon-square.svg -resize 180x180 \
  PNG24:../static/dashboard/apple-touch-icon.png
cp icon.svg ../static/dashboard/favicon.svg
cp icon-32.png ../static/dashboard/favicon-32.png
```

`icon.ico` (the Windows installer and its shortcut) holds the 16 to 256 px PNG
files above, side by side in one ICO file. Make it again after a change:

```sh
python3 - <<'PY'
import struct
sizes = [16, 32, 48, 64, 128, 256]
pngs = [open(f'icon-{s}.png', 'rb').read() for s in sizes]
head = struct.pack('<HHH', 0, 1, len(sizes))
offset, entries = 6 + 16 * len(sizes), b''
for s, p in zip(sizes, pngs):
    w = 0 if s >= 256 else s
    entries += struct.pack('<BBBBHHII', w, w, 0, 0, 1, 32, len(p), offset)
    offset += len(p)
open('icon.ico', 'wb').write(head + entries + b''.join(pngs))
PY
```
