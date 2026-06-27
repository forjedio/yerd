#!/bin/sh
# Yerd GUI .deb post-remove.
#
# Remove any /usr/bin symlinks postinst created — but ONLY on real removal, never
# on upgrade (the new package's postinst recreates them). In the normal layout
# Tauri ships the real binaries in /usr/bin (dpkg removes those itself); postinst
# only creates symlinks in the /usr/lib fallback path. dpkg does not track
# maintainer-script-created symlinks, so this is the only cleanup. The -L guard
# means real, dpkg-owned files (and files another package owns) are left alone.
set -e

case "$1" in
  remove|purge)
    for b in yerd yerdd yerd-helper; do
      [ -L "/usr/bin/$b" ] && rm -f "/usr/bin/$b"
    done
    ;;
esac

exit 0
