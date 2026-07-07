#!/bin/sh
# Yerd GUI .rpm %postun scriptlet.
#
# Remove any /usr/bin symlinks %post created - but ONLY on real erase, never on
# upgrade (the new package's %post recreates them). In the normal layout Tauri
# ships the real binaries in /usr/bin (rpm removes those itself); %post only
# creates symlinks in the /usr/lib fallback path. rpm does not track
# scriptlet-created symlinks, so this is the only cleanup. We remove a symlink
# only if it still points into the /usr/lib fallback dir (the exact target shape
# %post creates) - so real rpm-owned files and foreign symlinks another package
# repointed are left alone, mirroring %post's refuse-to-clobber guard.
#
# Unlike a deb postrm (which switches on a remove/purge/upgrade verb), an rpm
# %postun receives $1 = the count of this package's versions remaining after the
# transaction: 0 on a final erase, 1 during an upgrade. Clean up only when it hits
# 0, so an upgrade (whose new %post will recreate the symlinks) is left untouched.
set -e

if [ "$1" = 0 ]; then
  for b in yerd yerdd yerd-helper; do
    [ -L "/usr/bin/$b" ] || continue
    case "$(readlink "/usr/bin/$b")" in
      /usr/lib/*/"$b") rm -f "/usr/bin/$b" ;;
    esac
  done
fi

exit 0
