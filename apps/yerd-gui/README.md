# yerd-gui

The Yerd desktop app: **Tauri v2 + Vue 3 (`<script setup>`) + TypeScript +
Tailwind**, styled with hand-rolled shadcn-vue-style components.

It is a **thin `yerd-ipc` client of the `yerdd` daemon** — exactly like the CLI.
The `src-tauri` Rust layer only maps each Tauri command to one IPC `Request`
(`src-tauri/src/commands.rs` → `ipc.rs`); all behaviour lives in the daemon and
its crates. Features that need a daemon IPC which doesn't exist yet (log
viewing, daemon restart, per-service restart) render as disabled **"Coming
soon"** affordances rather than being faked client-side.

## Layout

```
src/              Vue app — ipc/ (typed client + wire-JSON types), composables/,
                  components/ (ui/ primitives), views/ (PhpView, SitesView,
                  ServicesView, AboutView)
src-tauri/        Rust bridge — ipc.rs, commands.rs, elevate.rs, error.rs, main.rs
scripts/          install-dev-desktop.sh (dev taskbar/dock icon on Linux)
```

## Prerequisites

### Toolchain
- **Rust ≥ 1.85** (edition2024). The workspace libraries still target the 1.77
  MSRV, but current Tauri v2 (`tauri-utils`, plugins) pulls `toml 1.x` /
  `serde_spanned 1.x`, which need edition2024. `rust-toolchain.toml` pins
  **1.96.0** for this reason.
- **Node 22+** + npm (this host uses `fnm`; the binary lives at
  `~/.local/share/fnm/node-versions/v22.*/installation/bin`).

### Linux system `-dev` packages (Debian/Ubuntu)
Building Tauri from source needs the GTK/WebKit/tray dev headers (the runtime
libs alone aren't enough). Install once:

```sh
sudo apt install \
  libwebkit2gtk-4.1-dev libgtk-3-dev libsoup-3.0-dev \
  libjavascriptcoregtk-4.1-dev libayatana-appindicator3-dev \
  libdbus-1-dev libxdo-dev librsvg2-dev build-essential pkg-config
```

(`libdbus-1-dev` is required by `tauri-plugin-single-instance` + the
appindicator tray; `libxdo-dev` by the tray input layer.)

## Develop / build / test

```sh
npm install            # JS deps
npm run dev            # Vite dev server (frontend only)
npm run tauri dev      # full app: webview + Rust bridge (start `cargo run -p yerdd` first)
npm run build          # vue-tsc type-check + vite production build
npm run test           # vitest (frontend unit/component tests)

cargo test -p yerd-gui # Rust bridge unit tests (needs the -dev packages above)
```

On Linux, run `scripts/install-dev-desktop.sh` once so the dev window gets the
Yerd icon in the taskbar/dock (Wayland/GNOME/Pantheon match the icon via a
`.desktop` file, which a packaged build ships but `tauri dev` doesn't).

## Notes / follow-ups
- **Install the CLI alongside the GUI.** The app shells out to `yerd` for
  privileged "Fix" actions and needs `yerdd` running. Releases ship the GUI
  bundles (`.dmg`/`.AppImage`/`.deb`) *and* the CLI `.deb`/tarballs separately;
  on Linux both `.deb`s install to `/usr/bin` (siblings), which the elevation
  path expects. A self-contained bundle that **embeds** the CLI via Tauri
  `externalBin` is a planned follow-up.
- **In-app elevation**: Linux uses `pkexec /usr/bin/env SUDO_UID=<uid> <yerd>
  elevate <t>`; macOS uses `osascript … with administrator privileges` wrapping
  the same `env SUDO_UID=<uid> <yerd> elevate <t>`. The macOS daemon socket lives
  at the deterministic `/tmp/yerd-$UID` so the root-elevated CLI can locate it
  from `SUDO_UID` alone.
- **Windows** is out of scope: the daemon's pipe name isn't client-derivable yet.
- macOS release bundles are **Developer ID signed and notarised**, so they open
  without a Gatekeeper prompt (signing/notarisation is wired up on this branch).
