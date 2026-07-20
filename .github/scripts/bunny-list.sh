#!/usr/bin/env bash
#
# Recursively list a Bunny Storage prefix as a JSON array of files.
#
# Usage: bunny-list.sh <remote-dir>            (e.g. releases/)
# Output (stdout):
#   [ {"path":"releases/<tag>/<name>","size":N,"checksum":"HEX"|null}, ... ]
#
# Bunny's Storage list endpoint is NOT recursive, so this walks each
# subdirectory itself. `Checksum` is an uppercase SHA-256 that Bunny populates
# only for objects it hashed (null otherwise); it is emitted verbatim and the
# consumer case-folds / falls back to size. Directories are omitted (files only).
#
# Requires: BUNNY_STORAGE_ACCESS_KEY, BUNNY_STORAGE_ZONE, BUNNY_STORAGE_ENDPOINT
set -euo pipefail

start=${1:?usage: bunny-list.sh <remote-dir>}

: "${BUNNY_STORAGE_ACCESS_KEY:?BUNNY_STORAGE_ACCESS_KEY is not set}"
: "${BUNNY_STORAGE_ZONE:?BUNNY_STORAGE_ZONE is not set}"
: "${BUNNY_STORAGE_ENDPOINT:?BUNNY_STORAGE_ENDPOINT is not set (region host)}"

# Normalise to "<prefix>/" with no leading slash.
start=${start#/}
[ -z "$start" ] || [ "${start: -1}" = "/" ] || start="${start}/"

base="https://${BUNNY_STORAGE_ENDPOINT}/${BUNNY_STORAGE_ZONE}"

# List one directory (trailing-slash, leading-slash-free path). Emits one
# compact JSON object per FILE to stdout and recurses into subdirectories. A 404
# (prefix does not exist yet) is treated as empty.
list_dir() {
  local dir=$1 resp code body
  resp=$(curl -sS -w $'\n%{http_code}' --connect-timeout 30 --max-time 120 \
    -H "AccessKey: ${BUNNY_STORAGE_ACCESS_KEY}" "${base}/${dir}")
  code=${resp##*$'\n'}
  body=${resp%$'\n'*}
  case "$code" in
    200) ;;
    404) return 0 ;;
    *) echo "::error::bunny-list: HTTP $code listing ${dir}" >&2; exit 1 ;;
  esac

  # Iterate entries; recurse into dirs, emit files with a zone-root-relative path.
  local obj isdir name
  while IFS= read -r obj; do
    [ -n "$obj" ] || continue
    isdir=$(printf '%s' "$obj" | jq -r '.IsDirectory')
    name=$(printf '%s' "$obj" | jq -r '.ObjectName')
    if [ "$isdir" = "true" ]; then
      list_dir "${dir}${name}/"
    else
      printf '%s' "$obj" | jq -c --arg path "${dir}${name}" \
        '{path: $path, size: .Length, checksum: (.Checksum // null)}'
    fi
  done < <(printf '%s' "$body" | jq -c '.[]')
}

list_dir "$start" | jq -s '.'
