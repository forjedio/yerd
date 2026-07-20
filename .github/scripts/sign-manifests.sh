#!/usr/bin/env bash
#
# Sign one or more CDN root manifests (latest.json / releases.json) with the
# release minisign key and verify each signature against the public key the
# daemon EMBEDS, so a manifest can never ship a signature the shipped binary
# cannot check. The daemon verifies latest.json against
# `yerd_update::UPDATE_PUBLIC_KEY` before trusting it (see
# `self_update::fetch_releases`), the same key that signs the release artifacts.
#
# Usage: sign-manifests.sh <file> [<file> ...]
#   Writes <file>.minisig next to each input.
#
# Requires: MINISIGN_SECRET_KEY (an *unencrypted* key, as the release job uses,
# to avoid a CI password prompt) and GITHUB_WORKSPACE (to read the embedded
# public key from the source tree). RUNNER_TEMP is used for the transient key
# file when set, falling back to a mktemp dir otherwise.
set -euo pipefail

[ "$#" -ge 1 ] || { echo "::error::sign-manifests: no files given" >&2; exit 1; }
: "${MINISIGN_SECRET_KEY:?MINISIGN_SECRET_KEY is not set - cannot sign the manifests}"
: "${GITHUB_WORKSPACE:?GITHUB_WORKSPACE is not set}"

tmp=${RUNNER_TEMP:-$(mktemp -d)}

# `minisign` isn't in the runner's apt repos, so install the official prebuilt
# static binary from the canonical (pinned) release over HTTPS if it is not
# already on PATH (the release job may have installed it earlier in the run).
if ! command -v minisign >/dev/null 2>&1; then
  MINISIGN_VERSION=0.11
  curl -sSfL "https://github.com/jedisct1/minisign/releases/download/${MINISIGN_VERSION}/minisign-${MINISIGN_VERSION}-linux.tar.gz" \
    | tar -xz -C "$tmp"
  minisign_bin=$(find "$tmp/minisign-linux" -path '*x86_64*' -name minisign | head -n1)
  [ -n "$minisign_bin" ] || { echo "::error::minisign x86_64 binary not found in release tarball" >&2; exit 1; }
  sudo install -m0755 "$minisign_bin" /usr/local/bin/minisign
fi
command -v minisign >/dev/null

# The trust anchor: read the key the daemon embeds and refuse to sign with a
# secret whose public half is the placeholder test key (a mismatched secret
# would produce signatures the shipped binary rejects).
art="${GITHUB_WORKSPACE}/crates/yerd-update/src/artifact.rs"
key=$(grep -oE 'UPDATE_PUBLIC_KEY: &str = "[^"]+"' "$art" | sed -E 's/.*"([^"]+)".*/\1/')
[ -n "$key" ] || { echo "::error::could not read UPDATE_PUBLIC_KEY from $art" >&2; exit 1; }
test_key="RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3"
if [ "$key" = "$test_key" ]; then
  echo "::error::UPDATE_PUBLIC_KEY is still the placeholder test key - wire the production key before signing" >&2
  exit 1
fi

# Wipe the key on ANY exit (incl. a mid-loop failure), not just success.
keyfile="${tmp}/minisign-manifest.key"
trap 'rm -f "$keyfile"' EXIT
umask 077; printf '%s\n' "$MINISIGN_SECRET_KEY" > "$keyfile"

for f in "$@"; do
  [ -f "$f" ] || { echo "::error::sign-manifests: no such file: $f" >&2; exit 1; }
  # -H = prehashed, which `minisign-verify` (and the daemon) require.
  minisign -S -H -s "$keyfile" -m "$f" -x "$f.minisig"
  minisign -V -H -P "$key" -m "$f" -x "$f.minisig" \
    || { echo "::error::embedded UPDATE_PUBLIC_KEY does not verify $f.minisig (key/secret mismatch)" >&2; exit 1; }
  echo "signed + verified $f -> $f.minisig"
done
