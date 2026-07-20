#!/usr/bin/env bash
#
# Purge one or more pull-zone URLs from Bunny's edge cache, so a freshly
# overwritten root manifest is visible immediately instead of serving a cached
# copy.
#
# Usage: bunny-purge.sh <url> [<url> ...]
#
# Requires: BUNNY_PURGE_API_KEY (an account-scoped Bunny API key - broader than
# the storage-zone key; provision it as its own secret and scope/rotate it as
# narrowly as Bunny allows).
set -euo pipefail

: "${BUNNY_PURGE_API_KEY:?BUNNY_PURGE_API_KEY is not set}"
[ "$#" -ge 1 ] || { echo "usage: bunny-purge.sh <url> [<url> ...]" >&2; exit 1; }

for u in "$@"; do
  encoded=$(jq -rn --arg u "$u" '$u | @uri')
  curl -fsS --retry 3 --retry-connrefused --retry-delay 2 \
    -X POST -H "AccessKey: ${BUNNY_PURGE_API_KEY}" \
    "https://api.bunny.net/purge?url=${encoded}&async=false"
  echo "PURGE $u"
done
