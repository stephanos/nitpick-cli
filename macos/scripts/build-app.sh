#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MACOS_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
ROOT_DIR="$(cd "$MACOS_DIR/.." && pwd)"

APP_NAME="Nitpick Agent"
APP_DIR="$ROOT_DIR/target/macos/$APP_NAME.app"
CONTENTS_DIR="$APP_DIR/Contents"
MACOS_CONTENTS_DIR="$CONTENTS_DIR/MacOS"
FRAMEWORKS_DIR="$CONTENTS_DIR/Frameworks"
RESOURCES_DIR="$CONTENTS_DIR/Resources"

mkdir -p "$MACOS_CONTENTS_DIR" "$FRAMEWORKS_DIR" "$RESOURCES_DIR"

cargo build --release -p nitpick-agent-cli -p nitpick-agent-host
mise exec -- swift build --package-path "$MACOS_DIR" -c release --product NitpickAgentApp

/usr/bin/ditto "$MACOS_DIR/Bundle/Info.plist" "$CONTENTS_DIR/Info.plist"
/usr/bin/ditto "$MACOS_DIR/.build/release/NitpickAgentApp" "$MACOS_CONTENTS_DIR/$APP_NAME"
/usr/bin/install_name_tool -add_rpath "@executable_path/../Frameworks" "$MACOS_CONTENTS_DIR/$APP_NAME" 2>/dev/null || true
/usr/bin/ditto "$ROOT_DIR/target/release/nitpick-agent" "$MACOS_CONTENTS_DIR/nitpick-agent"
/usr/bin/ditto "$ROOT_DIR/target/release/nitpick-agent-host" "$MACOS_CONTENTS_DIR/nitpick-agent-host"
/usr/bin/ditto "$MACOS_DIR/Resources" "$RESOURCES_DIR"

SPARKLE_FRAMEWORK="$(
  find "$MACOS_DIR/.build" -path "*/Sparkle.framework" -type d -print -quit 2>/dev/null || true
)"

if [[ -n "$SPARKLE_FRAMEWORK" ]]; then
  /usr/bin/ditto "$SPARKLE_FRAMEWORK" "$FRAMEWORKS_DIR/Sparkle.framework"
else
  echo "warning: Sparkle.framework was not found under $MACOS_DIR/.build" >&2
fi

if [[ -n "${CODESIGN_IDENTITY:-}" ]]; then
  if /usr/bin/grep -q "REPLACE_WITH_SPARKLE_EDDSA_PUBLIC_KEY\\|https://example.com/nitpick-agent/appcast.xml" "$CONTENTS_DIR/Info.plist"; then
    echo "refusing to sign app with placeholder Sparkle configuration" >&2
    exit 1
  fi
  codesign --force --deep --options runtime --sign "$CODESIGN_IDENTITY" "$APP_DIR"
else
  echo "skipping code signing; set CODESIGN_IDENTITY for distributable builds" >&2
fi

echo "$APP_DIR"
