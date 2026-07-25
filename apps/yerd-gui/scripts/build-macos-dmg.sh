#!/usr/bin/env bash
# Package the styled macOS .dmg headlessly with `appdmg` — no Finder, no
# AppleScript, no Automation/TCC permission dependency. Tauri's own dmg
# bundler (create-dmg + Finder scripting) has proven unreliable both locally
# (fails outright: "Not authorised to send Apple events to Finder") and in CI
# (succeeds but silently skips the background/icon-position styling), so
# `dmg` was removed from tauri.conf.json's bundle.targets and this script
# builds the dmg as a separate step after Tauri produces just the `.app`.
#
# Every path is resolved off this script's own location, never off the
# invocation cwd, so it works identically whether run from the CI job's
# default `apps/yerd-gui` cwd or a developer's repo-root shell.
#
# Usage: ./build-macos-dmg.sh
# Env:   APPLE_SIGNING_IDENTITY (optional) — codesigns the dmg if set;
#        without it, produces an unsigned dmg for local testing.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GUI_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
REPO_ROOT="$(cd "$GUI_DIR/../.." && pwd)"

# Shared with the CI "Verify macOS signing" / updater-tarball steps in
# .github/workflows/build.yml — single source of truth for the search roots.
source "$SCRIPT_DIR/lib/find-app.sh"

build_roots "$REPO_ROOT/target" "$GUI_DIR/src-tauri/target"
[ "${#ROOTS[@]}" -gt 0 ] || { echo "::error::no build output dir found under $REPO_ROOT/target or $GUI_DIR/src-tauri/target"; exit 1; }

APP=$(find_yerd_app)
[ -n "$APP" ] || { echo "::error::Yerd.app not found under ${ROOTS[*]} (run \`npm run tauri build\` first)"; exit 1; }

BUNDLE_DIR="$(dirname "$(dirname "$APP")")"   # .../bundle
OUT_DIR="$BUNDLE_DIR/dmg"
mkdir -p "$OUT_DIR"

VERSION=$(TAURI_CONF="$GUI_DIR/src-tauri/tauri.conf.json" node -p "require(process.env.TAURI_CONF).version")
OUT="$OUT_DIR/Yerd_${VERSION}_aarch64.dmg"

# A real temp dir (not mktemp-plus-string-concat, which orphans the original
# mktemp file) with a trap so it's cleaned up on every exit path, including
# a failure below.
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT
RESOLVED_JSON="$TMP_DIR/resolved.json"

# Paths passed via env vars, not spliced into the JS source, so a checkout
# path containing a quote or backslash can't break the generated script.
DMG_DIR="$GUI_DIR/src-tauri/dmg" APP_PATH="$APP" RESOLVED_OUT="$RESOLVED_JSON" node -e "
const fs = require('fs');
const path = require('path');
const dmgDir = process.env.DMG_DIR;
const spec = JSON.parse(fs.readFileSync(path.join(dmgDir, 'appdmg.json'), 'utf8'));
spec.background = path.join(dmgDir, spec.background);
spec.icon = path.join(dmgDir, spec.icon);
for (const item of spec.contents) {
  if (item.path === '__APP__') item.path = process.env.APP_PATH;
}
fs.writeFileSync(process.env.RESOLVED_OUT, JSON.stringify(spec, null, 2));
"

rm -f "$OUT"

# Absolute path, not bare \`npx\` — npx resolves node_modules relative to the
# invocation cwd, which would break when this script isn't run from $GUI_DIR.
APPDMG="$GUI_DIR/node_modules/.bin/appdmg"

# appdmg's "Unmounting temporary image" step runs \`hdiutil detach\` on the
# volume it mounted ten steps earlier, but on macOS CI runners
# diskarbitrationd/Spotlight often auto-unmounts the image in that window, so
# the detach fails with ENOENT ("No such file or directory"). appdmg only
# retries detach on EBUSY (exit code 16), not ENOENT, so it aborts and removes
# the half-built dmg. Each run starts clean, so a whole-command retry clears
# the transient failure. Read the volume title from the resolved spec (env, not
# spliced into JS) so our own failed mount can be force-detached before each
# retry, keeping the next attempt from being bumped onto "/Volumes/<title> 1".
# Only detached on retry, never before the first run, so a developer's mounted
# Yerd installer volume is left alone on the happy path.
VOL_TITLE=$(RESOLVED_JSON="$RESOLVED_JSON" node -p "require(process.env.RESOLVED_JSON).title")

detach_stale_volumes() {
  local vol
  for vol in "/Volumes/$VOL_TITLE" "/Volumes/$VOL_TITLE "*; do
    [ -d "$vol" ] || continue
    hdiutil detach -force "$vol" >/dev/null 2>&1 || true
  done
}

attempt=0
max_attempts=4
until "$APPDMG" "$RESOLVED_JSON" "$OUT"; do
  attempt=$((attempt + 1))
  if [ "$attempt" -ge "$max_attempts" ]; then
    echo "::error::appdmg failed after $max_attempts attempts (last exit non-zero)"
    exit 1
  fi
  echo "::warning::appdmg attempt $attempt failed; cleaning up and retrying"
  rm -f "$OUT"
  detach_stale_volumes
  sleep $((attempt * 3))
done

if [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
  codesign --force --sign "$APPLE_SIGNING_IDENTITY" --timestamp "$OUT"
  codesign --verify --strict --verbose=2 "$OUT"
fi

echo "$OUT"
