#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
export BREW_PREFIX="${BREW_PREFIX:-$(brew --prefix)}"
export GPHOTO_PREFIX="${GPHOTO_PREFIX:-$(brew --prefix libgphoto2)}"
SIGNING_ARGS=()
if [[ -n "${APPLE_SIGNING_IDENTITY:-}" ]]; then
  : "${APPLE_TEAM_ID:?APPLE_TEAM_ID is required for release signing}"
  SIGNING_ARGS=(--signing-identity "$APPLE_SIGNING_IDENTITY" --team-id "$APPLE_TEAM_ID")
fi
exec python3 "$SCRIPT_DIR/macos_runtime.py" assemble \
  "$GPHOTO_PREFIX" "${GPHOTO_OUTPUT:-$SCRIPT_DIR/../src-tauri/resources/gphoto}" \
  ${SIGNING_ARGS[@]+"${SIGNING_ARGS[@]}"}
