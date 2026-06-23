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
src-tauri/        Rust bridge — main.rs, commands.rs, ipc.rs, error.rs, elevate.rs,
                  daemon.rs (resolve/start/stop + install-CLI-on-PATH),
                  autostart.rs (per-user service + run-at-login),
                  smappservice.rs + mac_trust.rs (macOS), mail_window.rs
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

### Release bundle (single self-contained artifact)

The GUI is the **only** shipped artifact: it **embeds all three binaries**
(`yerd`, `yerdd`, `yerd-helper`) via Tauri `externalBin`, signed/notarized inside
the one bundle on macOS. There is **no** standalone CLI tarball/`.deb` and **no**
runtime download. On macOS the daemon registers as a background
[`SMAppService`](src-tauri/src/smappservice.rs) agent (so *Login Items → Allow in
the Background* shows **Yerd**, not the signing team).

The embedding lives in **per-platform release overlays** applied only to the
release build (a plain `tauri dev` / `tauri build` skips them — so dev needs no
staged sidecars):

```sh
# 1) Build the three binaries and stage them as Tauri sidecars (git-ignored dir).
cargo build --release -p yerd -p yerdd -p yerd-helper
mkdir -p src-tauri/binaries
# macOS:
for b in yerd yerdd yerd-helper; do
  cp ../../target/release/$b src-tauri/binaries/$b-aarch64-apple-darwin
done
# Linux: use the -x86_64-unknown-linux-gnu suffix instead.

# 2) Build with the platform overlay (externalBin + macOS plist / Linux postinst).
#    `--config` is resolved relative to this dir, so the path is src-tauri/-relative.
npm run tauri build -- --config src-tauri/tauri.bundle-macos.conf.json   # macOS
# npm run tauri build -- --config src-tauri/tauri.bundle-linux.conf.json # Linux
```

Without step 1 a release build fails (`externalBin` not found). CI does both
steps automatically (`.github/workflows/release.yml`, `gui` job). The macOS floor
is **13** (`SMAppService`). On **Linux** the `.deb` `postinst` symlinks the three
binaries from `/usr/lib/<product>/` into `/usr/bin` (so `yerd` is on PATH and the
GUI/CLI resolve their siblings) and `setcap`s `yerdd` for ports 80/443. AppImage
is **not** built (its ephemeral mount can't persist `setcap`).

**Terminal CLI:** Linux gets `/usr/bin/yerd` from the `.deb`; macOS offers
*Settings → Terminal CLI → Install yerd on your PATH* (symlinks the bundled `yerd`
into `{data}/bin` via `yerd path install`).

**Uninstall (macOS):** macOS has no app-uninstall hook, so the background
registration persists after you delete `Yerd.app`. Turn off *Run the Yerd daemon
in the background* first (or remove the **Yerd** entry under System Settings →
Login Items → Allow in the Background).

## Notes / follow-ups
- **Single artifact.** The app embeds `yerd`/`yerdd`/`yerd-helper`; the GUI shells
  out to the sibling `yerd` for privileged "Fix" actions and supervises `yerdd`.
  No separate CLI `.deb`/tarball is published.
- **In-app elevation**: Linux uses `pkexec /usr/bin/env SUDO_UID=<uid> <yerd>
  elevate <t>`; macOS uses `osascript … with administrator privileges` wrapping
  the same `env SUDO_UID=<uid> <yerd> elevate <t>`. The macOS daemon socket lives
  at the deterministic `/tmp/yerd-$UID` so the root-elevated CLI can locate it
  from `SUDO_UID` alone.
- **Windows** is out of scope: the daemon's pipe name isn't client-derivable yet.
- macOS release bundles are **Developer ID signed and notarised**, so they open
  without a Gatekeeper prompt (signing/notarisation is wired up on this branch).
