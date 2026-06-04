#!/usr/bin/env bash
# Regenerate every icon asset from scratch.
#
#   ./scripts/build-icons.sh
#
# Requires: python3, and (for the raster assets) librsvg2-bin (rsvg-convert),
# icoutils (icotool), and icnsutils (png2icns):
#   sudo apt-get install -y librsvg2-bin icoutils icnsutils
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
assets="$root/assets"
svg="$assets/icon.svg"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# 1. Source SVG.
python3 "$root/scripts/gen_icon.py"

# 2. Rasterize to the sizes we need.
sizes=(16 24 32 48 64 128 256 512 1024)
for s in "${sizes[@]}"; do
  rsvg-convert -w "$s" -h "$s" "$svg" -o "$tmp/icon-$s.png"
done

# 3. Canonical PNG (used for the README and the runtime window icon).
cp "$tmp/icon-512.png" "$assets/icon.png"

# 4. Windows multi-size .ico (embedded into the .exe via build.rs).
icotool -c -o "$assets/icon.ico" \
  "$tmp/icon-16.png" "$tmp/icon-24.png" "$tmp/icon-32.png" \
  "$tmp/icon-48.png" "$tmp/icon-64.png" "$tmp/icon-128.png" "$tmp/icon-256.png"

# 5. macOS .icns (used by the .app bundle).
png2icns "$assets/icon.icns" \
  "$tmp/icon-16.png" "$tmp/icon-32.png" "$tmp/icon-48.png" \
  "$tmp/icon-128.png" "$tmp/icon-256.png" "$tmp/icon-512.png" "$tmp/icon-1024.png"

# 6. Android launcher icons (legacy mipmaps, referenced by cargo-apk as
#    @mipmap/ic_launcher via [package.metadata.android]).
res="$assets/android-res"
gen_mipmap() { # density-name  size
  local d="$res/mipmap-$1"
  mkdir -p "$d"
  rsvg-convert -w "$2" -h "$2" "$svg" -o "$d/ic_launcher.png"
}
gen_mipmap mdpi 48
gen_mipmap hdpi 72
gen_mipmap xhdpi 96
gen_mipmap xxhdpi 144
gen_mipmap xxxhdpi 192

echo "Generated:"
ls -la "$assets"
find "$res" -type f | sort
