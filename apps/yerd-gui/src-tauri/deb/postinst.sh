#!/bin/sh
# Yerd GUI .deb post-install.
#
# Tauri installs the embedded sidecars (yerd, yerdd, yerd-helper) under
# /usr/lib/<product>/ — NOT /usr/bin — so they are not siblings of the GUI's
# /usr/bin/yerd-gui. We symlink them into /usr/bin so that:
#   * the GUI's trusted_yerd() (sibling of yerd-gui) finds /usr/bin/yerd,
#   * `yerd` is on the user's PATH for terminal use,
#   * current_exe() canonicalizes through the symlink so `yerd`'s sibling lookup
#     resolves yerdd/yerd-helper in the real embedded dir.
# Then grant the daemon permission to bind privileged ports (80/443); if that
# fails (overlayfs/NFS/noxattr mounts can't hold file capabilities) the daemon
# falls back to 8080/8443. Re-runs on every upgrade (dpkg wipes file caps and the
# maintainer-script symlinks are not dpkg-tracked, so recreate idempotently).
set -e

case "$1" in
  configure|abort-upgrade|abort-deconfigure|abort-remove) ;;
  *) exit 0 ;;
esac

# Locate the single embedded dir holding all three binaries. Fail closed on none;
# refuse on an ambiguous match (a stale/foreign tree).
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
if [ -z "$dir" ]; then
  echo "yerd: could not locate the embedded yerd binaries under /usr/lib" >&2
  exit 1
fi

# Co-locate on PATH; -sfn force-recreates every configure (self-healing).
for b in yerd yerdd yerd-helper; do
  ln -sfn "$dir/$b" "/usr/bin/$b"
done

# Privileged-port capability on the REAL binary; best-effort.
if command -v setcap >/dev/null 2>&1; then
  setcap 'cap_net_bind_service=+ep' "$dir/yerdd" \
    || echo "yerd: setcap failed; the daemon will use ports 8080/8443" >&2
else
  echo "yerd: setcap not found (install libcap2-bin); the daemon will use 8080/8443" >&2
fi

exit 0
