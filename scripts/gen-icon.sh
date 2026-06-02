#!/usr/bin/env bash
# Regenerate the macOS app icon (.icns) + window icon (256px) from the source
# SVG. macOS-only: uses qlmanage (WebKit) to rasterize the SVG, then sips +
# iconutil. Run when assets/icon/icon.svg changes.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ICONDIR="$ROOT/crates/kompass-bin/assets/icon"
SVG="$ICONDIR/icon.svg"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

echo "==> rasterizing $SVG"
qlmanage -t -s 1024 -o "$TMP" "$SVG" >/dev/null 2>&1
sips -z 1024 1024 "$TMP/icon.svg.png" --out "$TMP/icon_1024.png" >/dev/null

ISET="$TMP/Kompass.iconset"
mkdir -p "$ISET"
for s in 16 32 128 256 512; do
  sips -z "$s" "$s" "$TMP/icon_1024.png" --out "$ISET/icon_${s}x${s}.png" >/dev/null
done
sips -z 32 32   "$TMP/icon_1024.png" --out "$ISET/icon_16x16@2x.png"   >/dev/null
sips -z 64 64   "$TMP/icon_1024.png" --out "$ISET/icon_32x32@2x.png"   >/dev/null
sips -z 256 256 "$TMP/icon_1024.png" --out "$ISET/icon_128x128@2x.png" >/dev/null
sips -z 512 512 "$TMP/icon_1024.png" --out "$ISET/icon_256x256@2x.png" >/dev/null
sips -z 1024 1024 "$TMP/icon_1024.png" --out "$ISET/icon_512x512@2x.png" >/dev/null

iconutil -c icns "$ISET" -o "$ICONDIR/Kompass.icns"
cp "$ISET/icon_256x256.png" "$ICONDIR/icon_256.png"
echo "==> wrote $ICONDIR/Kompass.icns + icon_256.png"
