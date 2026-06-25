#!/usr/bin/env bash
#
# release.sh — cut a Yerd release.
#
# Usage:
#   ./scripts/release.sh --version v2.0.2-rc.5
#   ./scripts/release.sh --version 2.0.3
#   ./scripts/release.sh --version v2.0.3 --dry-run     # print, change nothing
#   ./scripts/release.sh --version v2.0.3 --tag-only    # push only the tag, not the branch
#
# The version is normalised to canonical semver: a leading `v` is stripped and
# the result must match  MAJOR.MINOR.PATCH[-prerelease]  — the exact shape the
# release workflow's tag filter accepts. `v2.0.2.rc-5` is rejected; you want
# `v2.0.2-rc.5`.

set -euo pipefail

# ---------------------------------------------------------------------------
# Output helpers
# ---------------------------------------------------------------------------
if [ -t 1 ]; then
  BOLD=$(printf '\033[1m'); RED=$(printf '\033[31m'); GRN=$(printf '\033[32m')
  YLW=$(printf '\033[33m'); BLU=$(printf '\033[34m'); RST=$(printf '\033[0m')
else
  BOLD=; RED=; GRN=; YLW=; BLU=; RST=
fi
step() { printf '%s==>%s %s%s%s\n' "$BLU" "$RST" "$BOLD" "$*" "$RST"; }
info() { printf '    %s\n' "$*"; }
ok()   { printf '%s ok %s %s\n' "$GRN" "$RST" "$*"; }
warn() { printf '%swarn%s %s\n' "$YLW" "$RST" "$*" >&2; }
die()  { printf '%serror%s %s\n' "$RED" "$RST" "$*" >&2; exit 1; }

usage() {
  sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'
  exit "${1:-0}"
}

# ---------------------------------------------------------------------------
# Parse arguments
# ---------------------------------------------------------------------------
RAW_VERSION=""
REMOTE="origin"
DRY_RUN=0
TAG_ONLY=0

while [ $# -gt 0 ]; do
  case "$1" in
    --version)   RAW_VERSION="${2:-}"; shift 2 ;;
    --version=*) RAW_VERSION="${1#*=}"; shift ;;
    --remote)    REMOTE="${2:-}"; shift 2 ;;
    --remote=*)  REMOTE="${1#*=}"; shift ;;
    --dry-run)   DRY_RUN=1; shift ;;
    --tag-only)  TAG_ONLY=1; shift ;;
    -h|--help)   usage 0 ;;
    *)           die "unknown argument: $1  (try --help)" ;;
  esac
done

[ -n "$RAW_VERSION" ] || die "--version is required  (e.g. --version v2.0.2-rc.5)"

# Strip a single leading `v`, then validate canonical semver.
VERSION="${RAW_VERSION#v}"
TAG="v${VERSION}"

if ! printf '%s' "$VERSION" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$'; then
  die "invalid version '$RAW_VERSION'
    expected  MAJOR.MINOR.PATCH[-prerelease]  (a leading 'v' is fine), e.g.
      v2.0.3        final release
      v2.0.2-rc.5   prerelease   (note: '-rc.5', not '.rc-5')
    this is the only shape the release.yml tag filter accepts."
fi

# ---------------------------------------------------------------------------
# Locate the repo and sanity-check git state
# ---------------------------------------------------------------------------
command -v cargo >/dev/null 2>&1 || die "cargo not found on PATH"
command -v git   >/dev/null 2>&1 || die "git not found on PATH"

ROOT=$(git rev-parse --show-toplevel 2>/dev/null) || die "not inside a git repository"
cd "$ROOT"

BRANCH=$(git rev-parse --abbrev-ref HEAD)
[ "$BRANCH" != "HEAD" ] || die "detached HEAD — check out a branch before releasing"

# The Release commit must contain only the version bump, so demand a clean tree
# (untracked files are tolerated; staged/unstaged changes are not).
if ! git diff --quiet || ! git diff --cached --quiet; then
  die "working tree has uncommitted changes — commit or stash them first"
fi

# Refuse to clobber an existing tag locally or on the remote.
if git rev-parse -q --verify "refs/tags/${TAG}" >/dev/null; then
  die "tag ${TAG} already exists locally"
fi
if git ls-remote --exit-code --tags "$REMOTE" "refs/tags/${TAG}" >/dev/null 2>&1; then
  die "tag ${TAG} already exists on remote '${REMOTE}'"
fi

# ---------------------------------------------------------------------------
# Summary / dry-run gate
# ---------------------------------------------------------------------------
step "Releasing ${BOLD}${TAG}${RST}"
info "version : ${VERSION}"
info "branch  : ${BRANCH}"
info "remote  : ${REMOTE}"
info "push    : $( [ "$TAG_ONLY" -eq 1 ] && echo 'tag only' || echo 'branch + tag' )"

if [ "$DRY_RUN" -eq 1 ]; then
  warn "--dry-run: no files, commits, tags, or pushes will be created"
  step "Would run: cargo run -p xtask -- bump ${VERSION}"
  step "Would run: cargo update --workspace"
  step "Would run: cargo run -p xtask -- version-check ${TAG}"
  step "Would commit version bump as: Release: ${TAG}"
  step "Would tag ${TAG} and push to ${REMOTE}"
  ok "dry run complete"
  exit 0
fi

# ---------------------------------------------------------------------------
# 1. Bump the manifests
# ---------------------------------------------------------------------------
step "Bumping manifests to ${VERSION}"
cargo run -q -p xtask -- bump "$VERSION"

# 2. Refresh the workspace crate versions in Cargo.lock (deps untouched). Not
#    fatal if it can't reach the registry — the build regenerates the lock — but
#    we prefer to commit a consistent lockfile.
step "Refreshing Cargo.lock"
if ! cargo update --workspace >/dev/null 2>&1; then
  warn "could not refresh Cargo.lock automatically (offline?); continuing"
fi

# 3. Verify every manifest matches the tag — the same gate CI runs.
step "Verifying manifests match ${TAG}"
cargo run -q -p xtask -- version-check "$TAG"

# ---------------------------------------------------------------------------
# 4. Commit the bump
# ---------------------------------------------------------------------------
step "Committing version bump"
MANIFESTS=(
  "Cargo.toml"
  "Cargo.lock"
  "apps/yerd-gui/src-tauri/tauri.conf.json"
  "apps/yerd-gui/package.json"
)
git add -- "${MANIFESTS[@]}"

if git diff --cached --quiet; then
  die "nothing staged after bump — is the version already ${VERSION}?"
fi

git commit -m "Release: ${TAG}" >/dev/null
ok "committed: Release: ${TAG}"

# ---------------------------------------------------------------------------
# 5. Tag
# ---------------------------------------------------------------------------
step "Tagging ${TAG}"
git tag -a "$TAG" -m "Release: ${TAG}"
ok "created tag ${TAG}"

# ---------------------------------------------------------------------------
# 6. Push
# ---------------------------------------------------------------------------
if [ "$TAG_ONLY" -eq 1 ]; then
  warn "pushing tag only — '${BRANCH}' will not advance on '${REMOTE}'"
  step "Pushing tag ${TAG} to ${REMOTE}"
  git push "$REMOTE" "refs/tags/${TAG}"
else
  step "Pushing ${BRANCH} to ${REMOTE}"
  git push "$REMOTE" "$BRANCH"
  step "Pushing tag ${TAG} to ${REMOTE}"
  git push "$REMOTE" "refs/tags/${TAG}"
fi

ok "released ${TAG}"
