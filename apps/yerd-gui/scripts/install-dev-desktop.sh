#!/usr/bin/env bash
# Install a *development* .desktop entry + themed icons so the Linux taskbar/dock
# shows the Yerd mark while running `npm run tauri dev`.
#
# Why this is needed: on Linux (especially Wayland — GNOME/Pantheon), the dock
# takes a window's icon from a .desktop file whose name/StartupWMClass matches
# the window's app_id (here `yerd-gui`, the dev binary name). It ignores the
# X11 `_NET_WM_ICON` that Tauri's `set_icon` sets. A packaged `.deb` ships this
# desktop file automatically; this script reproduces it for local dev.
#
# Re-run is idempotent. Remove with: rm ~/.local/share/applications/yerd-gui.desktop
set -euo pipefail

APP_ID="yerd-gui"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"   # apps/yerd-gui
ICONS="$HERE/src-tauri/icons"
APPS_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
DESKTOP="$APPS_DIR/$APP_ID.desktop"

mkdir -p "$APPS_DIR"

# Install icons into the user hicolor theme under the app_id name.
for size in 32 64 128 256; do
  src="$ICONS/${size}x${size}.png"
  [ "$size" = 128 ] && src="$ICONS/128x128.png"
  if [ -f "$src" ]; then
    xdg-icon-resource install --noupdate --mode user --context apps \
      --size "$size" "$src" "$APP_ID"
  fi
done
xdg-icon-resource forceupdate --mode user 2>/dev/null || true

cat > "$DESKTOP" <<DESKTOP_EOF
[Desktop Entry]
Type=Application
Name=Yerd (dev)
Comment=Local PHP development environment — dev build
Icon=$APP_ID
Exec=sh -c 'cd "$HERE" && npm run tauri dev'
Terminal=false
Categories=Development;
StartupWMClass=$APP_ID
DESKTOP_EOF

update-desktop-database "$APPS_DIR" 2>/dev/null || true

echo "Installed $DESKTOP (Icon=$APP_ID, StartupWMClass=$APP_ID)."
echo "Restart the app window (close it, then \`npm run tauri dev\`) so the"
echo "compositor re-associates it with the new desktop entry."
