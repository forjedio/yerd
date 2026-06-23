#!/usr/bin/env sh
# Yerd no longer ships a standalone CLI/daemon. Everything — the daemon, the
# `yerd` CLI, and the privileged helper — is bundled inside the desktop app.
#
# Install Yerd from the GitHub Releases page instead:
#
#   macOS:  download Yerd_MacOS_AppleSilicon_v<ver>.dmg, open it, drag Yerd to Applications
#   Linux:  download Yerd_Linux_x86_64_v<ver>.deb, then:  sudo apt install ./Yerd_Linux_x86_64_*.deb
#
#   https://github.com/forjedio/yerd/releases/latest
#
# The `yerd` terminal command comes with the app: on Linux the .deb puts it on
# PATH; on macOS use Settings → Terminal CLI → "Install yerd on your PATH".
set -eu

echo "Yerd is now distributed as a single desktop app — there is no separate CLI installer."
echo
echo "Download it from: https://github.com/forjedio/yerd/releases/latest"
echo "  macOS: Yerd_MacOS_AppleSilicon_v<ver>.dmg"
echo "  Linux: Yerd_Linux_x86_64_v<ver>.deb  (sudo apt install ./Yerd_Linux_x86_64_*.deb)"
exit 0
