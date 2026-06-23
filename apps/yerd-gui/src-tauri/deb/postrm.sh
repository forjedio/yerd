#!/bin/sh
# Yerd GUI .deb post-remove.
#
# Remove the /usr/bin symlinks postinst created — but ONLY on real removal, never
# on upgrade (the new package's postinst recreates them). dpkg does not track
# maintainer-script-created symlinks, so this is the only cleanup. Guard on -L so
# we never delete a real file another package legitimately owns.
set -e

case "$1" in
  remove|purge)
    for b in yerd yerdd yerd-helper; do
      [ -L "/usr/bin/$b" ] && rm -f "/usr/bin/$b"
    done
    ;;
esac

exit 0
