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
#
# Env knobs (optional): PURGE_ATTEMPTS (whole-request retries per URL, default 5)
# and PURGE_BACKOFF (seconds, grows linearly per attempt, default 3). curl's own
# --retry handles network/5xx blips; this outer loop rides out Bunny's per-URL
# purge throttling (HTTP 429), which curl does not always retry.
set -euo pipefail

: "${BUNNY_PURGE_API_KEY:?BUNNY_PURGE_API_KEY is not set}"
[ "$#" -ge 1 ] || { echo "usage: bunny-purge.sh <url> [<url> ...]" >&2; exit 1; }

attempts="${PURGE_ATTEMPTS:-5}"
backoff="${PURGE_BACKOFF:-3}"

# Purge one URL, retrying the whole request up to $attempts times with a linear
# backoff. Returns 0 as soon as any attempt succeeds, non-zero if all are spent.
purge_one() {
  local u="$1" encoded try
  encoded=$(jq -rn --arg u "$u" '$u | @uri')
  for ((try = 1; try <= attempts; try++)); do
    if curl -fsS --retry 3 --retry-connrefused --retry-delay 2 \
      --connect-timeout 30 --max-time 120 \
      -X POST -H "AccessKey: ${BUNNY_PURGE_API_KEY}" \
      "https://api.bunny.net/purge?url=${encoded}&async=false"; then
      echo "PURGE $u (attempt ${try}/${attempts})"
      return 0
    fi
    if [ "$try" -lt "$attempts" ]; then
      local delay=$((backoff * try))
      echo "retry $u in ${delay}s (attempt ${try}/${attempts} failed)" >&2
      sleep "$delay"
    fi
  done
  return 1
}

# Purges are idempotent and independent per URL, so attempt every one even if an
# earlier URL exhausts its retries - a single flaky purge must not skip the rest.
# Collect the URLs that never succeeded and exit non-zero at the end so the caller
# still sees the error (the release job is continue-on-error, so this surfaces in
# the summary without failing the release).
failed=()
for u in "$@"; do
  purge_one "$u" || { echo "FAILED $u" >&2; failed+=("$u"); }
done

if [ "${#failed[@]}" -gt 0 ]; then
  echo "::error::bunny-purge: ${#failed[@]} of $# purge(s) failed after ${attempts} attempts: ${failed[*]}" >&2
  exit 1
fi
