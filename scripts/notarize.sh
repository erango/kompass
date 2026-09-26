#!/usr/bin/env bash
# Notarize and staple a Developer ID-signed Kompass.app or .dmg.
#   ./scripts/notarize.sh dist/Kompass.app
#   ./scripts/notarize.sh Kompass-1.2.3.dmg
#
# Credentials (first match wins):
#   KOMPASS_NOTARY_PROFILE   keychain profile created once with
#                            `xcrun notarytool store-credentials <name> ...`
#   APPLE_API_KEY_ID + APPLE_API_ISSUER_ID + APPLE_API_KEY_PATH
#                            App Store Connect API key (.p8) — used in CI
set -euo pipefail

TARGET="${1:?usage: notarize.sh <path to .app or .dmg>}"
[ -e "$TARGET" ] || { echo "not found: $TARGET" >&2; exit 1; }

if [ -n "${KOMPASS_NOTARY_PROFILE:-}" ]; then
  AUTH=(--keychain-profile "$KOMPASS_NOTARY_PROFILE")
elif [ -n "${APPLE_API_KEY_ID:-}" ]; then
  AUTH=(--key "${APPLE_API_KEY_PATH:?}" --key-id "$APPLE_API_KEY_ID" --issuer "${APPLE_API_ISSUER_ID:?}")
else
  echo "no notarization credentials (set KOMPASS_NOTARY_PROFILE or APPLE_API_KEY_*)" >&2
  exit 1
fi

# notarytool takes a zip/dmg/pkg, not a bare .app — zip it for submission,
# but staple the ticket to the original bundle.
SUBMIT="$TARGET"
if [[ "$TARGET" == *.app ]]; then
  SUBMIT="$(mktemp -d)/$(basename "$TARGET" .app).zip"
  ditto -c -k --keepParent "$TARGET" "$SUBMIT"
fi

echo "==> submitting $(basename "$SUBMIT") for notarization…"
OUT="$(xcrun notarytool submit "$SUBMIT" "${AUTH[@]}" --wait --output-format json)"
ID="$(plutil -extract id raw -o - - <<<"$OUT")"
STATUS="$(plutil -extract status raw -o - - <<<"$OUT")"
echo "==> $ID: $STATUS"
if [ "$STATUS" != "Accepted" ]; then
  xcrun notarytool log "$ID" "${AUTH[@]}" || true
  exit 1
fi

xcrun stapler staple "$TARGET"
xcrun stapler validate "$TARGET"
echo "==> notarized + stapled: $TARGET"
