#!/usr/bin/env bash
#
# Delete one object from a Bunny Storage zone.
#
# Usage: bunny-delete.sh <remote-path>
#   <remote-path> is relative to the storage-zone root, e.g.
#   releases/v1.0.0/OLD.deb
#
# Requires: BUNNY_STORAGE_ACCESS_KEY, BUNNY_STORAGE_ZONE, BUNNY_STORAGE_ENDPOINT
set -euo pipefail

remote=${1:?usage: bunny-delete.sh <remote-path>}

: "${BUNNY_STORAGE_ACCESS_KEY:?BUNNY_STORAGE_ACCESS_KEY is not set}"
: "${BUNNY_STORAGE_ZONE:?BUNNY_STORAGE_ZONE is not set}"
: "${BUNNY_STORAGE_ENDPOINT:?BUNNY_STORAGE_ENDPOINT is not set (region host)}"

# Refuse anything outside releases/ - deletion must never touch builds/ or the
# root manifests, whatever the caller passes. A `..` segment or a leading slash
# is rejected up front: without that, `releases/../latest.json` passes the glob
# but curl's default path normalisation would collapse the `..` and issue the
# DELETE against /latest.json, escaping releases/.
case "$remote" in
  /*) echo "::error::bunny-delete: refusing absolute path: $remote" >&2; exit 1 ;;
  *..*) echo "::error::bunny-delete: refusing path with '..': $remote" >&2; exit 1 ;;
  releases/*) ;;
  *) echo "::error::bunny-delete: refusing to delete outside releases/: $remote" >&2; exit 1 ;;
esac

url="https://${BUNNY_STORAGE_ENDPOINT}/${BUNNY_STORAGE_ZONE}/${remote}"

# --path-as-is: send the literal (guard-checked) path; do not let curl normalise
# away any dot segment, belt-and-suspenders alongside the `..` rejection above.
curl -fsS --retry 3 --retry-connrefused --retry-delay 2 --path-as-is \
  -X DELETE -H "AccessKey: ${BUNNY_STORAGE_ACCESS_KEY}" "$url"

echo "DELETE $remote"
