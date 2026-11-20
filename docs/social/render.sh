#!/bin/sh
# Re-render the Open Graph card and the Twitter/X header from their HTML
# sources into docs/public/images. Requires Google Chrome (macOS path below;
# override with CHROME=...). Both use the Sites screenshot from the docs.
set -eu
cd "$(dirname "$0")"
CHROME="${CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
render() {
  "$CHROME" --headless=new --disable-gpu --hide-scrollbars \
    --allow-file-access-from-files --force-device-scale-factor=1 \
    --window-size="$2" --virtual-time-budget=3000 \
    --screenshot="$PWD/../public/images/$1.png" "file://$PWD/$1.html"
}
render social-card 1200,630
render twitter-header 1500,500
