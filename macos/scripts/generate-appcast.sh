#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MACOS_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
ROOT_DIR="$(cd "$MACOS_DIR/.." && pwd)"

APP_NAME="Nitpick Agent"
APP_DIR="$ROOT_DIR/target/macos/$APP_NAME.app"
ARCHIVES_DIR="${SPARKLE_ARCHIVES_DIR:-$ROOT_DIR/target/sparkle}"
DOWNLOAD_URL_PREFIX="${SPARKLE_DOWNLOAD_URL_PREFIX:-https://github.com/stephanos/nitpick-agent/releases/latest/download/}"
KEYCHAIN_ACCOUNT="${SPARKLE_KEYCHAIN_ACCOUNT:-nitpick-agent}"
GENERATE_APPCAST="$MACOS_DIR/.build/artifacts/sparkle/Sparkle/bin/generate_appcast"

if [[ -z "${CODESIGN_IDENTITY:-}" ]]; then
  echo "CODESIGN_IDENTITY is required because Sparkle rejects unsigned update archives" >&2
  exit 1
fi

VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$MACOS_DIR/Bundle/Info.plist")"
BUILD="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$MACOS_DIR/Bundle/Info.plist")"
ARCHIVE_NAME="Nitpick-Agent-$VERSION-$BUILD.zip"

"$SCRIPT_DIR/build-app.sh"

mkdir -p "$ARCHIVES_DIR"
rm -f "$ARCHIVES_DIR/$ARCHIVE_NAME"
/usr/bin/ditto -c -k --keepParent "$APP_DIR" "$ARCHIVES_DIR/$ARCHIVE_NAME"

if [[ -n "${SPARKLE_PRIVATE_ED_KEY:-}" ]]; then
  printf '%s' "$SPARKLE_PRIVATE_ED_KEY" | "$GENERATE_APPCAST" \
    --ed-key-file - \
    --download-url-prefix "$DOWNLOAD_URL_PREFIX" \
    --maximum-versions 1 \
    "$ARCHIVES_DIR"
elif [[ -n "${SPARKLE_PRIVATE_ED_KEY_FILE:-}" ]]; then
  "$GENERATE_APPCAST" \
    --ed-key-file "$SPARKLE_PRIVATE_ED_KEY_FILE" \
    --download-url-prefix "$DOWNLOAD_URL_PREFIX" \
    --maximum-versions 1 \
    "$ARCHIVES_DIR"
else
  "$GENERATE_APPCAST" \
    --account "$KEYCHAIN_ACCOUNT" \
    --download-url-prefix "$DOWNLOAD_URL_PREFIX" \
    --maximum-versions 1 \
    "$ARCHIVES_DIR"
fi

echo "$ARCHIVES_DIR/appcast.xml"
