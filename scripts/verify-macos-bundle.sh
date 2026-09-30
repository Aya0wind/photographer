#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

APP_PATH="${1:-src-tauri/target/aarch64-apple-darwin/release/bundle/macos/Photo Hub.app}"
if [[ $# -ge 2 ]]; then
  DMG_PATH="$2"
else
  DMG_DIR="src-tauri/target/aarch64-apple-darwin/release/bundle/dmg"
  DMG_COUNT="$(find "$DMG_DIR" -maxdepth 1 -type f -name '*.dmg' -print | wc -l | tr -d ' ')"
  [[ "$DMG_COUNT" -eq 1 ]] || {
    echo "期望恰好一个 DMG，实际找到 $DMG_COUNT" >&2
    find "$DMG_DIR" -maxdepth 1 -type f -name '*.dmg' -print >&2
    exit 1
  }
  DMG_PATH="$(find "$DMG_DIR" -maxdepth 1 -type f -name '*.dmg' -print)"
fi

BIN_PATH="$APP_PATH/Contents/MacOS/smart-photo"
GPHOTO_ROOT="$APP_PATH/Contents/Resources/gphoto"

[[ -d "$APP_PATH" ]] || { echo "app 不存在: $APP_PATH" >&2; exit 1; }
[[ -f "$BIN_PATH" ]] || { echo "app 可执行文件不存在: $BIN_PATH" >&2; exit 1; }
[[ -f "$DMG_PATH" ]] || { echo "dmg 不存在: $DMG_PATH" >&2; exit 1; }

ARCH="$(lipo -archs "$BIN_PATH")"
[[ "$ARCH" == "arm64" ]] || { echo "期望 arm64，实际: $ARCH" >&2; exit 1; }

: "${APPLE_TEAM_ID:?Expected APPLE_TEAM_ID is required for release verification}"
python3 "$SCRIPT_DIR/macos_runtime.py" audit "$GPHOTO_ROOT" --team-id "$APPLE_TEAM_ID"

codesign --verify --deep --strict --verbose=2 "$APP_PATH"
# An ad-hoc signature can pass codesign verification but is not a release.
SIGNATURE="$(codesign --display --verbose=4 "$APP_PATH" 2>&1)"
grep -q '^Authority=Developer ID Application:' <<< "$SIGNATURE"
grep -Eq '^TeamIdentifier=[A-Z0-9]+$' <<< "$SIGNATURE"
grep -Fxq "TeamIdentifier=$APPLE_TEAM_ID" <<< "$SIGNATURE"
grep -q '^Timestamp=' <<< "$SIGNATURE"
grep -q 'flags=.*(runtime)' <<< "$SIGNATURE"
hdiutil verify "$DMG_PATH"
file "$BIN_PATH" "$DMG_PATH"
echo "macOS bundle audit passed: $APP_PATH"
