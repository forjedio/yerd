#!/usr/bin/env sh
# Yerd CLI installer — downloads the latest release's CLI binaries (the daemon +
# `yerd` + `yerd-helper`), verifies them against SHA256SUMS, and installs them.
#
#   curl -fsSL https://raw.githubusercontent.com/forjedio/yerd/main/scripts/install.sh | sh
#
# Debian/Ubuntu (dpkg + apt present) → installs the .deb (system-wide, sudo).
# Everything else (other Linux, macOS) → installs the tarball to ~/.local/bin
# (no sudo). Override with env: YERD_VERSION=2.0.2, YERD_BIN_DIR=~/bin,
# YERD_REPO=owner/repo.
#
# This installs the CLI/daemon. The desktop GUI ships as separate
# .dmg/.AppImage/.deb bundles — see the README.
set -eu

REPO="${YERD_REPO:-forjedio/yerd}"
API="https://api.github.com/repos/${REPO}"

say() { printf '%s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "'$1' is required but not found"; }

need curl
need tar
command -v sha256sum >/dev/null 2>&1 || command -v shasum >/dev/null 2>&1 \
  || die "need 'sha256sum' or 'shasum' to verify downloads"

os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
  Linux)  os_id=linux ;;
  Darwin) os_id=darwin ;;
  *) die "unsupported OS '$os' — Yerd supports Linux and macOS" ;;
esac
case "$arch" in
  x86_64|amd64)  rust_arch=x86_64;  deb_arch=amd64 ;;
  aarch64|arm64) rust_arch=aarch64; deb_arch=arm64 ;;
  *) die "unsupported architecture '$arch'" ;;
esac
if [ "$os_id" = linux ]; then
  triple="${rust_arch}-unknown-linux-gnu"
else
  triple="${rust_arch}-apple-darwin"
fi

# Resolve the version (latest stable unless pinned via YERD_VERSION). Note the
# GitHub `latest` endpoint excludes prereleases, so it 404s if only prereleases
# exist — point the user at YERD_VERSION rather than dying cryptically.
version="${YERD_VERSION:-}"
if [ -z "$version" ]; then
  latest_json="$(curl -fsSL "${API}/releases/latest")" \
    || die "no published stable release for ${REPO} — pin one with YERD_VERSION=<x.y.z>"
  version="$(printf '%s\n' "$latest_json" \
    | grep -o '"tag_name":[[:space:]]*"[^"]*"' | head -1 \
    | sed -E 's/^"tag_name":[[:space:]]*"v?//; s/"$//')"
fi
[ -n "$version" ] || die "could not determine the version to install"
tag="v${version}"
base="https://github.com/${REPO}/releases/download/${tag}"
say "Installing Yerd ${version} for ${os_id}/${rust_arch}"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM

curl -fsSL "${base}/SHA256SUMS" -o "${tmp}/SHA256SUMS" \
  || die "could not fetch SHA256SUMS for ${tag}"

verify() {
  f="$1"; name="$(basename "$f")"
  # Exact filename match on field 2 (sha256sum's "<hash>  <name>"), so no
  # regex-metachar or partial-name surprises.
  expected="$(awk -v n="$name" '$2 == n { print $1 }' "${tmp}/SHA256SUMS")"
  [ -n "$expected" ] || die "no checksum listed for ${name}"
  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$f" | awk '{print $1}')"
  else
    actual="$(shasum -a 256 "$f" | awk '{print $1}')"
  fi
  [ "$expected" = "$actual" ] || die "checksum mismatch for ${name}"
  say "  verified ${name}"
}

fetch() { curl -fsSL "${base}/$1" -o "${tmp}/$1" || die "download failed: $1"; }

if [ "$os_id" = linux ] && command -v dpkg >/dev/null 2>&1 && command -v apt-get >/dev/null 2>&1; then
  need sudo
  asset="yerd_${version}_${deb_arch}.deb"
  fetch "$asset"; verify "${tmp}/${asset}"
  say "Installing ${asset} (requires sudo)…"
  sudo dpkg -i "${tmp}/${asset}" || sudo apt-get -f install -y
  say ""
  say "Installed. Start the per-user daemon:"
  say "  systemctl --user enable --now yerd"
else
  asset="yerd-${version}-${triple}.tar.gz"
  fetch "$asset"; verify "${tmp}/${asset}"
  dest="${YERD_BIN_DIR:-$HOME/.local/bin}"
  mkdir -p "$dest"
  tar -C "$tmp" -xzf "${tmp}/${asset}"
  for b in yerd yerdd yerd-helper; do
    install -m 0755 "${tmp}/${b}" "${dest}/${b}"
  done
  say ""
  say "Installed yerd, yerdd, yerd-helper to ${dest}"
  case ":${PATH}:" in
    *":${dest}:"*) : ;;
    *) say "  (add ${dest} to your PATH)";;
  esac

  # systemd user service for non-Debian distros (Arch/Omarchy, Fedora, …),
  # with ExecStart pointed at the actual install dir.
  if [ "$os_id" = linux ] && command -v systemctl >/dev/null 2>&1 \
     && [ -f "${tmp}/systemd/yerd.service" ]; then
    unit_dir="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
    mkdir -p "$unit_dir"
    sed "s|^ExecStart=.*|ExecStart=${dest}/yerdd serve|" \
      "${tmp}/systemd/yerd.service" > "${unit_dir}/yerd.service"
    say "Installed systemd user unit → ${unit_dir}/yerd.service"
    say "Start it:  systemctl --user daemon-reload && systemctl --user enable --now yerd"
  else
    say "Start the daemon (rootless, runs on 8080/8443):  yerdd serve &"
  fi
fi

say ""
say "One-time setup for the full experience (the only step that uses root):"
say "  sudo yerd elevate        # trust the local CA · route *.test · allow 80/443"
say "Then:  yerd park ~/Sites   →   http://<folder>.test"
