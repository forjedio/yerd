#!/usr/bin/env bash
#
# Upload one file to a Bunny Storage zone and verify it landed.
#
# Usage: bunny-put.sh <local-file> <remote-path>
#   <remote-path> is relative to the storage-zone root, e.g.
#   releases/v2.0.4/Yerd_Linux_x86_64_v2-0-4.deb
#
# Requires: BUNNY_STORAGE_ACCESS_KEY, BUNNY_STORAGE_ZONE, BUNNY_STORAGE_ENDPOINT
# (the region host, e.g. ny.storage.bunnycdn.com - a wrong/absent region host
# hard-401s indistinguishably from a bad key, so it is mandatory).
set -euo pipefail

local_file=${1:?usage: bunny-put.sh <local-file> <remote-path>}
remote=${2:?usage: bunny-put.sh <local-file> <remote-path>}

: "${BUNNY_STORAGE_ACCESS_KEY:?BUNNY_STORAGE_ACCESS_KEY is not set}"
: "${BUNNY_STORAGE_ZONE:?BUNNY_STORAGE_ZONE is not set}"
: "${BUNNY_STORAGE_ENDPOINT:?BUNNY_STORAGE_ENDPOINT is not set (region host, e.g. ny.storage.bunnycdn.com)}"

[ -f "$local_file" ] || { echo "::error::bunny-put: no such file: $local_file" >&2; exit 1; }

url="https://${BUNNY_STORAGE_ENDPOINT}/${BUNNY_STORAGE_ZONE}/${remote}"

# -T streams the file (implies PUT, sets Content-Length; no chunked, which Bunny
# rejects). -f makes any non-2xx a hard failure.
curl -fsS --retry 3 --retry-connrefused --retry-delay 2 \
  -T "$local_file" \
  -H "AccessKey: ${BUNNY_STORAGE_ACCESS_KEY}" \
  -H "Content-Type: application/octet-stream" \
  "$url"

# Bunny Storage has no HEAD method; a ranged GET with the AccessKey is the
# documented existence check.
curl -fsS -o /dev/null -r 0-0 -H "AccessKey: ${BUNNY_STORAGE_ACCESS_KEY}" "$url" \
  || { echo "::error::bunny-put: post-upload verify failed for $remote" >&2; exit 1; }

echo "PUT $remote"
