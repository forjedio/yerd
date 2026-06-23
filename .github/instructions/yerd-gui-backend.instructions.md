---
applyTo: "apps/yerd-gui/src-tauri/**/*.rs"
---

# yerd-gui (src-tauri) — the Tauri bridge

The Rust side of the desktop app: a **thin bridge** that turns Tauri commands
into `yerd-ipc` requests to the daemon. It is a client, exactly like the CLI.

**Layer:** thin bridge. No business logic — that lives in the daemon and its
crates.

## Owns

- One Tauri command per `yerd-ipc` `Request` (`commands.rs`): map
  `command → Request`, call the IPC `exchange`, convert a `Response::Error` into
  a typed `GuiError` so the frontend only ever sees success or a typed failure.
- The IPC client plumbing (`ipc.rs`) and host-only helpers.
- `elevate.rs` for triggering elevation flows where required.
- **Host-side daemon lifecycle** (`daemon.rs`, `autostart.rs`, `smappservice.rs`):
  resolve the bundled `yerd`/`yerdd`/`yerd-helper` (siblings of the app exe),
  start/stop the daemon via the per-user service, manage run-at-login (systemd
  `--user` on Linux; macOS **SMAppService** login-item registration), the
  optional "install the bundled `yerd` CLI on PATH", and macOS in-process CA
  trust (`mac_trust.rs`). These are host **orchestration**, not daemon logic —
  still no business rules here.

## Must not

- **Run as root.** The GUI process must never be privileged — this is a hard
  product rule. Privileged work goes through the daemon → `yerd-helper`.
- Embed or duplicate daemon logic, routing, supervision, or config authority.
- Add a match arm per `ErrorCode`/variant where serde can render the wire string
  generically — keep the bridge resilient to additive protocol changes.

## Conventions

- Keep commands mechanical: validate/convert inputs, exchange, map errors.
  Anything that looks like a decision belongs in a crate.
- Required Tauri lessons: use `tauri-plugin-single-instance` (duplicate launches
  otherwise spawn duplicate daemons); the Linux tray needs
  `libayatana-appindicator`; no Flatpak in v1 (tray bug). Distribution is
  whole-bundle releases (`.dmg`/`.deb` via the release workflow) — there is **no**
  in-app updater plugin; do not add one without a product decision.

## Review checklist

- [ ] No privileged execution in the GUI process.
- [ ] Commands are thin: `command → Request → exchange → typed result/GuiError`.
- [ ] No daemon logic duplicated here.
- [ ] Resilient to additive `yerd-ipc` changes (no brittle per-variant arms).
