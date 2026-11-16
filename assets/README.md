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
