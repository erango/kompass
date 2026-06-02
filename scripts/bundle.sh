#!/usr/bin/env bash
# Build Kompass.app (macOS) — no Xcode required.
#   ./scripts/bundle.sh              # release build, host arch (default)
#   ./scripts/bundle.sh --debug      # debug build (faster, for testing)
#   ./scripts/bundle.sh --universal  # release universal binary (arm64 + x86_64)
#   ./scripts/bundle.sh --open       # also launch the app when done
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROFILE="release"
PROFILE_DIR="release"
DO_OPEN=0
UNIVERSAL=0
for arg in "$@"; do
  case "$arg" in
    --debug) PROFILE="dev"; PROFILE_DIR="debug" ;;
    --open)  DO_OPEN=1 ;;
    --universal) UNIVERSAL=1 ;;
    *) echo "unknown arg: $arg" >&2; exit 1 ;;
  esac
done

VERSION="$(grep -m1 '^version' "$ROOT/Cargo.toml" | sed 's/.*"\(.*\)".*/\1/')"
APP="$ROOT/dist/Kompass.app"
ICNS="$ROOT/crates/kompass-bin/assets/icon/Kompass.icns"

# Always have an icon: (re)generate from the source SVG if it's missing or
# older than the SVG, so the bundle (and dock) always shows it.
SVG="$ROOT/crates/kompass-bin/assets/icon/icon.svg"
if [ ! -f "$ICNS" ] || [ "$SVG" -nt "$ICNS" ]; then
  echo "==> regenerating icon…"
  "$ROOT/scripts/gen-icon.sh"
fi

if [ "$UNIVERSAL" = "1" ]; then
  echo "==> building universal (arm64 + x86_64)…"
  rustup target add aarch64-apple-darwin x86_64-apple-darwin >/dev/null
  cargo build --manifest-path "$ROOT/Cargo.toml" -p kompass-bin --release --target aarch64-apple-darwin
  cargo build --manifest-path "$ROOT/Cargo.toml" -p kompass-bin --release --target x86_64-apple-darwin
  BIN="$ROOT/target/kompass-universal"
  lipo -create \
    "$ROOT/target/aarch64-apple-darwin/release/kompass" \
    "$ROOT/target/x86_64-apple-darwin/release/kompass" \
    -output "$BIN"
  echo "==> lipo: $(lipo -archs "$BIN")"
else
  echo "==> building ($PROFILE)…"
  cargo build --manifest-path "$ROOT/Cargo.toml" -p kompass-bin --profile "$PROFILE"
  BIN="$ROOT/target/$PROFILE_DIR/kompass"
fi

echo "==> assembling $APP (v$VERSION)…"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/Kompass"
cp "$ICNS" "$APP/Contents/Resources/Kompass.icns"

cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Kompass</string>
  <key>CFBundleDisplayName</key><string>Kompass</string>
  <key>CFBundleIdentifier</key><string>dev.kompass.Kompass</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleExecutable</key><string>Kompass</string>
  <key>CFBundleIconFile</key><string>Kompass</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
</dict>
</plist>
EOF

# Ad-hoc sign so the bundle is self-consistent (still "unidentified developer"
# on other Macs; real distribution needs a Developer ID cert + notarization).
codesign --force --deep --sign - "$APP" 2>/dev/null && echo "==> ad-hoc signed" || echo "==> codesign skipped"

touch "$APP"  # nudge the icon cache
echo "==> done: $APP"
[ "$DO_OPEN" = "1" ] && open "$APP"
exit 0
