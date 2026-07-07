#!/bin/sh
# Yerd GUI .deb post-remove.
#
# Remove any /usr/bin symlinks postinst created — but ONLY on real removal, never
# on upgrade (the new package's postinst recreates them). In the normal layout
# Tauri ships the real binaries in /usr/bin (dpkg removes those itself); postinst
# only creates symlinks in the /usr/lib fallback path. dpkg does not track
# maintainer-script-created symlinks, so this is the only cleanup. We remove a
# symlink only if it still points into the /usr/lib fallback dir (the exact target
# shape postinst creates) - so real dpkg-owned files and foreign symlinks another
# package repointed are left alone, mirroring postinst's refuse-to-clobber guard.
set -e

case "$1" in
  remove|purge)
    for b in yerd yerdd yerd-helper; do
      [ -L "/usr/bin/$b" ] || continue
      case "$(readlink "/usr/bin/$b")" in
        /usr/lib/*/"$b") rm -f "/usr/bin/$b" ;;
      esac
    done
    ;;
esac

exit 0
