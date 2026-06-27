#!/bin/sh
# Yerd GUI .deb post-install.
#
# Tauri's deb bundler copies the GUI **and** its externalBin sidecars (yerd,
# yerdd, yerd-helper) side-by-side into /usr/bin — so they are already on PATH
# and already siblings of /usr/bin/yerd-gui. No symlinking is needed in that
# (normal) layout; the only post-install work is granting the daemon permission
# to bind privileged ports (80/443). If setcap fails (overlayfs/NFS/noxattr
# mounts can't hold file capabilities) the daemon falls back to 8080/8443.
#
# A /usr/lib/<product>/ fallback is kept for resilience: older/foreign Tauri
# layouts staged the sidecars there instead of /usr/bin. In that case we symlink
# them onto PATH (the v1 behaviour) before setcap. Re-runs on every upgrade
# (dpkg wipes file caps and any maintainer-script symlinks aren't dpkg-tracked).
set -e

case "$1" in
  configure|abort-upgrade|abort-deconfigure|abort-remove) ;;
  *) exit 0 ;;
esac

# Locate the daemon: /usr/bin (normal) or a single /usr/lib/<dir>/ fallback we
# symlink onto PATH. Fail closed (below) only if it's absent from both.
yerdd=""
if [ -x /usr/bin/yerdd ] && [ -x /usr/bin/yerd ] && [ -x /usr/bin/yerd-helper ]; then
  yerdd=/usr/bin/yerdd
else
  # Locate the single embedded dir holding all three binaries; refuse on an
  # ambiguous match (a stale/foreign tree) before touching /usr/bin.
  dir=""
  for cand in /usr/lib/*/yerdd; do
    [ -f "$cand" ] || continue
    d=$(dirname "$cand")
    [ -f "$d/yerd" ] && [ -f "$d/yerd-helper" ] || continue
    if [ -n "$dir" ] && [ "$dir" != "$d" ]; then
      echo "yerd: multiple embedded binary dirs ($dir and $d); refusing to symlink" >&2
      exit 1
    fi
    dir="$d"
  done
  if [ -n "$dir" ]; then
    # Co-locate on PATH; refuse to clobber a real file or a foreign symlink at
    # /usr/bin/$b — that would steal a path owned by another package.
    for b in yerd yerdd yerd-helper; do
      src="$dir/$b"
      dst="/usr/bin/$b"
      if [ -e "$dst" ] && [ ! -L "$dst" ]; then
        echo "yerd: $dst exists and is not a symlink; refusing to overwrite" >&2
        exit 1
      fi
      if [ -L "$dst" ] && [ "$(readlink "$dst")" != "$src" ]; then
        echo "yerd: $dst points elsewhere; refusing to overwrite foreign symlink" >&2
        exit 1
      fi
      ln -sfn "$src" "$dst"
    done
    yerdd="$dir/yerdd"
  fi
fi
if [ -z "$yerdd" ]; then
  echo "yerd: could not locate the yerdd binary in /usr/bin or /usr/lib" >&2
  exit 1
fi

# Privileged-port capability on the REAL daemon binary; best-effort.
if command -v setcap >/dev/null 2>&1; then
  setcap 'cap_net_bind_service=+ep' "$yerdd" \
    || echo "yerd: setcap failed; the daemon will use ports 8080/8443" >&2
else
  echo "yerd: setcap not found (install libcap2-bin); the daemon will use 8080/8443" >&2
fi

exit 0
