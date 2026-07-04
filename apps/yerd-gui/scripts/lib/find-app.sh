#!/usr/bin/env bash
# Shared by build-macos-dmg.sh and the "Verify macOS signing" / "Package +
# verify macOS .app.tar.gz updater artifact" steps in
# .github/workflows/build.yml — single source of truth for locating the
# Tauri-built Yerd.app under the two possible bundle output roots (the repo
# workspace `target/`, or `apps/yerd-gui/src-tauri/target/` if Tauri used its
# own target dir). Source this file, then:
#   build_roots <candidate-dir> [<candidate-dir> ...]   # populates $ROOTS with the ones that exist
#   find_yerd_app                                        # echoes the Yerd.app path from $ROOTS, or empty

build_roots() {
  ROOTS=()
  local d
  for d in "$@"; do
    [ -d "$d" ] && ROOTS+=("$d")
  done
  # Under `set -e`, a function's exit status is that of its last command — the
  # final loop iteration's `[ -d ] && ...` test, which is legitimately false
  # (not present) as often as not. Calling this function as a bare statement
  # would then abort the caller even though "populate ROOTS" succeeded fine;
  # explicitly return 0 so only find_yerd_app's/caller's own checks matter.
  return 0
}

find_yerd_app() {
  # `|| true`: with 2+ matches, head closes the pipe and find takes SIGPIPE,
  # which pipefail would otherwise turn into a step/script abort.
  find "${ROOTS[@]}" -name 'Yerd.app' -type d 2>/dev/null | head -n1 || true
}
