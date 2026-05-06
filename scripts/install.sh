#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_NAME="Nitpick Agent"
APP_SOURCE="$ROOT_DIR/target/macos/$APP_NAME.app"
APP_DIR="${NITPICK_INSTALL_APP_DIR:-/Applications}"
APP_DEST="$APP_DIR/$APP_NAME.app"

"$ROOT_DIR/macos/scripts/build-app.sh"

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
echo "The app installs the nitpick-agent CLI into ~/.local/bin when it starts."
