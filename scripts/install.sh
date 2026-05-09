#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_NAME="Nitpick Agent"
APP_SOURCE="$ROOT_DIR/target/macos/$APP_NAME.app"
APP_DIR="${NITPICK_INSTALL_APP_DIR:-/Applications}"
APP_DEST="$APP_DIR/$APP_NAME.app"
BUNDLE_ID="com.stephanos.nitpick-agent"
HOST_PATTERN="$APP_DEST/Contents/MacOS/nitpick-agent-host daemon"

"$ROOT_DIR/macos/scripts/build-app.sh"

/usr/bin/osascript -e "tell application id \"$BUNDLE_ID\" to quit" >/dev/null 2>&1 || true
wait_until_stopped() {
  local pattern="$1"
  for _ in {1..20}; do
    if ! /usr/bin/pgrep -f "$pattern" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.25
  done
  return 1
}
wait_until_stopped "$APP_DEST/Contents/MacOS/$APP_NAME" || /usr/bin/pkill -f "$APP_DEST/Contents/MacOS/$APP_NAME" >/dev/null 2>&1 || true
wait_until_stopped "$HOST_PATTERN" || /usr/bin/pkill -f "$HOST_PATTERN" >/dev/null 2>&1 || true

mkdir -p "$APP_DIR"
rm -rf "$APP_DEST"
/usr/bin/ditto "$APP_SOURCE" "$APP_DEST"

if [[ "${NITPICK_INSTALL_SKIP_LAUNCH:-0}" != "1" ]]; then
  /usr/bin/open "$APP_DEST"
  launched=1
else
  launched=0
fi

echo "$APP_DEST"
if [[ "$launched" == "1" ]]; then
  echo "Launched Nitpick Agent.app."
else
  echo "Skipped launch because NITPICK_INSTALL_SKIP_LAUNCH=1."
fi
echo "The app installs the nitpick CLI into ~/.local/bin when it starts."
