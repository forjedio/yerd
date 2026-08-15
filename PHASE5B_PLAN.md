# Phase 5B Implementation Plan — Windows daemon autostart (per-user HKCU Run key)

Working doc (not committed). Companion to `WINDOWS_PLAN.md` Phase 5 and
`PHASE5_PLAN.md`. This plan **replaces `PHASE5_PLAN.md`'s Track B** with the
user-chosen option-(d) design from that doc's §2/D1: **per-user logon autostart
via an HKCU Run value**, per `WINDOWS_PLAN.md` locked decision #3 (revised
2026-08-03). Everything below was verified against the actual code on this
Windows machine on 2026-08-03.

**Dropped from the original Track B — none of this exists in 5B:**
no Windows Service, no SCM, no `windows-service` crate, no `--service`
dispatcher / `StartServiceCtrlDispatcher` / `define_windows_service!`, no
`sc.exe sdset`, no `pure/win_svc.rs` SDDL helper, no
`HelperInvocation::RegisterDaemonService`/`UnregisterDaemonService`, no
`ElevateTarget::Service`, no `SERVICE_STOP` Notify (Track A's `signals.rs`
rewrite landed without it and nothing needs it now), no service-mode entry
point in `yerdd`. Original steps B1–B5 are superseded by S1–S6 below.

**Explicit confirmations (the review checklist for this design):**

- **No `unsafe`.** `winreg` (safe crate, already a workspace dep) covers all
  registry access; `std::os::windows::process::CommandExt::creation_flags` is
  safe std API for the detached/hidden spawns. Nothing here needs FFI.
- **No SCM / no Windows Service** in any form (see the drop list above).
- **No elevation, no UAC, no helper op.** HKCU is the invoking user's own hive
  and spawning/killing the user's own processes needs no admin.
  `HelperInvocation` is untouched.
- **No `yerd-ipc` wire change** except ONE additive `DiagnosisCode` variant
  (S4), sanctioned by the enum's `#[non_exhaustive]` + the wire-pin test
  convention (`crates/yerd-ipc/src/status.rs:621`, `tests/wire_stability.rs:1270`).
- **macOS/Linux byte-identical.** Every change is `#[cfg(windows)]`-additive;
  the launchctl/systemctl arms of `yerd-service-ctl`, the GUI's `autostart.rs`,
  and the Unix uninstall path are not edited.
- **No new pinned dependencies.** `winreg 0.55` is already in
  `[workspace.dependencies]` (root `Cargo.toml:59`); 5B only adds
  `[target.'cfg(windows)'.dependencies]` references to it.

---

## 0. Ground truth (verified, not assumed)

### 0.1 What "daemon autostart" actually is on Unix (the parity target)

- **Registration is the GUI's job, not the CLI's and not a crate's.**
  `apps/yerd-gui/src-tauri/src/autostart.rs` owns enable/disable-at-login:
  Linux writes `~/.config/systemd/user/yerd.service` + `systemctl --user
  enable|disable yerd` (`write_unit`/`daemon_set_login`, lines 492/1258);
  macOS registers an SMAppService agent (bundled) or a loose LaunchAgent plist
  (`smapp_enable`/`ensure_bootstrapped`). The `not(linux|macos)` arm of
  `daemon_set_login` errors "not supported" — that arm is the Phase 6 GUI
  wiring point, deliberately untouched in Phase 5 (master plan).
- **The Unix per-user agent starts the daemon independently of the GUI.**
  The systemd `--user` unit (`WantedBy=default.target`) and the LaunchAgent
  (`RunAtLoad`) fire at login whether or not the GUI launches. GUI autostart
  ("Yerd" login item / `tauri-plugin-autostart`) is a separate, independent
  login entry. Concurrent starts are benign: the daemon is single-instance
  (`bin/yerdd/src/single_instance.rs` fs4 lock; the pipe is
  first-instance-exclusive, proven by
  `startup.rs::build_ipc_listener_binds_and_is_unique_per_dirs`).
- **`crates/yerd-service-ctl` is start/stop/restart only** (lib.rs, one file).
  The real surface is `ServiceCtl::{stop, start, restart}` — there is **no**
  enable/disable/status method on Unix:
  - `stop()` (lib.rs:78): best-effort `launchctl kill SIGTERM
    gui/$uid/dev.yerd.daemon` / `systemctl --user stop yerd`, **then**
    SIGTERM every `pgrep -x yerdd -U <uid>` pid (covers un-managed daemons).
    Infallible by signature (`fn stop(&self)`), errors swallowed.
  - `start()` (lib.rs:85): `launchctl kickstart -k` / `systemctl --user start
    yerd`, or, on Linux with no systemd user instance, a **detached spawn** of
    `<yerdd_path> serve` with null stdio + `process_group(0)` (lib.rs:181).
  - `restart()` (lib.rs:94): kickstart -k / `systemctl --user restart`, else
    Linux fallback `stop()` → bounded `wait_for_exit()` (50 × 100 ms pgrep
    poll, lib.rs:236) → `start()`.
  - The `not(macos|linux)` arms return `ServiceError::Unsupported`.
  - Stated crate design (module doc): shells out to platform tools, "no
    unsafe, no async, no IPC, no network"; deps are `thiserror` + Unix-only
    `nix`. Pure helpers (`parse_pids`) live in-file with table tests — the
    precedent 5B's pure helpers follow.
  - Sole current consumer: `bin/yerd/src/apply.rs:314,490` (self-update
    applier, itself `cfg(macos|linux)` — the Windows applier is Phase 6).
    `bin/yerd` already depends on the crate (`bin/yerd/Cargo.toml:25`).
- **Stopping is signal-based, not IPC.** There is no `Shutdown` request in
  `yerd-ipc` (`request.rs` has only `RestartDaemon`, line 268). So the
  faithful Windows mirror of "stop" is process termination, not a wire call —
  adding an IPC shutdown would be a forbidden wire change and *more* than
  Unix does.

### 0.2 How the GUI already autostarts on Windows

`tauri-plugin-autostart` → `auto-launch 0.5.0` (Cargo.lock:357). Its Windows
backend (registry source, `auto-launch-0.5.0/src/windows.rs`) writes
`HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Run`, value name = app name
(**`Yerd`**, from `tauri.conf.json` `productName`), data = `<exe path>
--autostarted`. It also maintains the Task-Manager override key
`...\CurrentVersion\Explorer\StartupApproved\Run` (12-byte REG_BINARY; entry
counts as enabled when the **last 8 bytes are all zero**,
`last_eight_bytes_all_zeros`). The GUI's Run entry launches only the GUI; on
Windows the GUI cannot start the daemon today (`plan_start`'s
`not(linux|macos)` arm errors) — that wiring is Phase 6.

### 0.3 Daemon process shape on Windows (already landed / in-flight Track A)

- `yerdd.exe` is a **console-subsystem** binary (no `windows_subsystem`
  attribute in `bin/yerdd/src/main.rs`). Launched from a Run entry it would
  open a **visible console window that stays for the daemon's lifetime** —
  this must be handled (D-5B.1).
- Logging survives a hidden console: `tracing_init.rs` always adds a
  daily-rolling file layer at `{cache}/yerdd.<date>.log` when dirs resolve
  (main.rs:23-24) — the same "stderr is discarded under launchd/systemd"
  rationale applies verbatim to a windowless login launch.
- `signals.rs` (Track A, already in the working tree) selects over Windows
  `ctrl_c` / `ctrl_break` / `ctrl_close` / `ctrl_shutdown`. **`ctrl_logoff`
  is not in the select** — relevant because a Run-key daemon dies at logoff
  and should drain gracefully (S2).
- Phase 2 Job Objects (`KILL_ON_JOB_CLOSE`) reap all php-cgi/DB/mail children
  whenever the daemon dies, however it dies — already relied on by
  `bin/yerd/src/uninstall.rs::stop_daemon` (windows_impl, line 182), which
  force-kills via absolute-path `taskkill /F /IM yerdd.exe`.
- The CLI↔daemon pipe works on Windows since Phase 1
  (`yerd-<sid>-<hash>` name, deny-probed DACL) — daemon reachability is
  already observable from any client.

### 0.4 Doctor today

- `yerd_doctor::diagnose(&StatusReport, path_needs_setup: Option<bool>)` —
  pure, `None`-probe-emits-nothing convention (`crates/yerd-doctor/src/lib.rs:49`).
  Called from exactly two places: `bin/yerdd/src/ipc_server.rs:235`
  (`Request::Diagnose`) and `:964` (`run_doctor_fix` re-diagnose).
- `DiagnosisCode` (`crates/yerd-ipc/src/status.rs:622`) is
  `#[non_exhaustive]`, snake_case wire tags, each variant pinned in
  `tests/wire_stability.rs::diagnosis_code_each_variant_byte_shape` (line 1270).
- **"Daemon running" is already checked** — client-side: when the CLI can't
  reach the daemon, `yerd doctor` renders a synthetic
  `DiagnosisCode::DaemonDown` FAIL (`bin/yerd/src/lib.rs:918`,
  `daemon_down_response`). This works on Windows today (Phase 1 transport).
  **No new code is needed for the "running" half of the check.**
- Track A's doctor step (PHASE5_PLAN A9) is scoped to `DaemonElevated` and the
  Windows `path_needs_setup` arm; it deliberately **excludes** the
  autostart-registered probe — that lands here (S4), additive alongside
  whatever A9 has merged by then.

### 0.5 CLI surface

`bin/yerd/src/cli.rs` has **no** daemon start/stop/autostart subcommand on any
OS (`Restart` is FPM pools; `Service Start/Stop` are DB services; nothing
drives `ServiceCtl` except the self-update applier). The Unix mirror is
therefore **"add nothing"**: introducing a Windows-only `yerd` autostart
command would exceed Unix parity. There is no `yerd elevate service` and 5B
adds no elevate target and no helper op — confirmed nothing else needs one.

---

## 1. Design

### 1.1 The mapping (Unix mechanism → Windows mirror)

| Concern | macOS | Linux | Windows (5B) |
|---|---|---|---|
| Enable at login | SMAppService / LaunchAgent plist (GUI) | systemd user unit + `enable` (GUI) | HKCU Run value (mechanics in `yerd-service-ctl`; GUI toggle wires in Phase 6) |
| Disable at login | unregister / `launchctl disable` | `systemctl --user disable` | delete the Run value |
| Start now | `launchctl kickstart -k` | `systemctl --user start`, else detached spawn | detached spawn of `yerdd.exe serve` (`CREATE_NO_WINDOW \| CREATE_NEW_PROCESS_GROUP`) |
| Stop now | `launchctl kill SIGTERM` + pgrep/SIGTERM | `systemctl --user stop` + pgrep/SIGTERM | absolute-path `taskkill /F /IM yerdd.exe` (Job Objects reap children; same trade `uninstall.rs` already ships) |
| Restart | kickstart -k | restart, else stop→wait→start | stop → bounded `tasklist` poll → start (mirrors the Linux no-systemd arm) |
| Registered probe | `smapp_registered()` / plist exists | unit file exists | Run value present ∧ StartupApproved not disabling |
| Running probe | pgrep (crate) / IPC (clients) | pgrep / IPC | clients: existing pipe ping (`DaemonDown`); crate-internal wait: `tasklist` CSV |
| Stops at | logoff | logoff | logoff (with graceful drain once `ctrl_logoff` is in the select, S2) |

### 1.2 Exact registry contract

- **Key:** `HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run`
- **Value name:** `Yerd Daemon` (constant `RUN_VALUE_NAME`; distinct from the
  GUI's `Yerd` value so the two entries never collide)
- **Type:** `REG_SZ`
- **Data:** `"<absolute path to yerdd.exe>" serve --detach`
  (path always quoted — profile paths like `C:\Users\John Smith\…` contain
  spaces; note `auto-launch` doesn't quote, which is a latent bug we do not
  copy). Rendered by a pure `run_value_data(yerdd: &Path) -> String` helper.
- **Task-Manager override (read + repair):**
  `HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run`,
  same value name, 12-byte `REG_BINARY`. Enabled ⇔ value absent OR last 8
  bytes all zero (pure `startup_approved_enabled(bytes: Option<&[u8]>) ->
  bool`, mirroring `auto-launch`). `enable_at_login` also rewrites this value
  to the enabled bytes when it exists, so re-enabling from Yerd beats a prior
  Task-Manager "Disable" — exactly what `auto-launch` does for the GUI entry.
- The Run data embeds the absolute `yerdd.exe` path; Phase 6's staged-rename
  self-update keeps that path stable (same master-plan invariant that keeps
  the `.cmd` shim wrappers valid), so the entry survives updates unmodified.
- `enable_at_login` is idempotent (`set_value` overwrites); `disable_at_login`
  tolerates an already-absent value.

### 1.3 GUI coordination (question 2) — dedicated daemon Run entry

Recommendation: **a dedicated `Yerd Daemon` Run entry, independent of the
GUI's `Yerd` entry** — because that is precisely the Unix shape (§0.1: the
user agent starts the daemon at login whether or not the GUI runs; GUI login
is a separate item). Relying on the GUI to spawn the daemon at login would
(a) diverge from Unix semantics, (b) couple "serve my sites at login" to
"open the app window at login", and (c) depend on Phase 6 GUI work that
doesn't exist yet. Double-launch is a non-issue: any second `yerdd` (GUI
start button, CLI, Run entry racing a manual start) fails fast on the
single-instance lock / exclusive pipe (§0.1) and exits; that is already the
accepted Unix behavior for GUI-kickstart-vs-RunAtLoad races. Phase 6's GUI
toggle (`daemon_set_login`'s `not(linux|macos)` arm) and `service_registered()`
/ `is_set_up` simply call the 5B `yerd-service-ctl` functions.

### 1.4 D-5B.1 — the console window (the one real wrinkle; decision to confirm)

`yerdd.exe` is a console binary, so a Run entry pointing straight at it would
leave a **persistent visible console window** for the daemon's whole lifetime.
That is unacceptable UX, and none of the zero-flash fixes fit the constraints
(GUI-subsystem `yerdd` breaks foreground dev use and console signals;
`conhost.exe --headless` is an undocumented flag; Task Scheduler was ruled out
with option d; wscript/VBS is deprecated cruft; PowerShell `-WindowStyle
Hidden` still flashes *and* adds quoting risk).

Chosen design: a hidden **`--detach` relauncher flag** on `yerdd serve`
(S2). The Run entry launches `yerdd.exe serve --detach`; the process
immediately respawns itself as `yerdd.exe serve` (original verbosity/config
flags preserved, `--detach` stripped) with
`CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP` + null stdio and exits 0. Cost:
a **brief console flash (~100–300 ms) at logon** — a known cosmetic wart to
document (same class as the `.cmd` "Terminate batch job" prompt), not solve.
`CREATE_NO_WINDOW` (hidden console) is deliberately chosen over
`DETACHED_PROCESS` (no console): a hidden console still receives console
control events, so logoff/shutdown reach `signals.rs` for a graceful drain.
Both flags are safe std (`creation_flags`); the constants are declared
locally (`const CREATE_NO_WINDOW: u32 = 0x0800_0000;`,
`CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;`).

Escalation note: if the human prefers **zero** flash over documented APIs,
the Run data becomes `%SystemRoot%\System32\conhost.exe --headless
"<yerdd>" serve` and S2's relauncher is dropped — flag for a human call;
default is the relauncher.

### 1.5 Where the code lives

All Run-key + process mechanics go in **`crates/yerd-service-ctl`** — its
module doc already names it "one place for the platform service mechanics so
the GUI, the `bin/yerd` self-update applier, and the uninstaller don't each
re-implement them". Pure decision helpers stay in-file with table tests
(`parse_pids` precedent), **not** in `yerd-platform/pure` — adding a
`yerd-platform` dep would explode this deliberately minimal crate's graph.
Consumers: `bin/yerd` (uninstall; already a dependent), `bin/yerdd` (doctor
probe; new `cfg(windows)` dep, lib←bin downhill, no cycle — the crate depends
on no `yerd-*` crate), Phase 6 GUI (later).

---

## 2. File-by-file checklist (ordered; workspace compiles on all three OSes after every step)

### S1. `crates/yerd-service-ctl` — the Windows backend

`Cargo.toml`:
```toml
[target.'cfg(windows)'.dependencies]
winreg = { workspace = true }
```

`src/lib.rs` (all additions `#[cfg(windows)]` unless noted; Unix code
byte-identical):

- Constants: `RUN_KEY = r"Software\Microsoft\Windows\CurrentVersion\Run"`,
  `STARTUP_APPROVED_KEY = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run"`,
  `RUN_VALUE_NAME = "Yerd Daemon"`, `DAEMON_EXE = "yerdd.exe"`.
- `ServiceError` gains an additive variant (enum is `#[non_exhaustive]`,
  crate-local, not wire): `#[error("registry access failed: {0}")]
  Registry(String)`.
- **`ServiceCtl` arms.** Re-gate the existing three-way bodies so Windows gets
  real arms and the fallback narrows to
  `#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]`:
  - `stop()`: `taskkill /F /IM yerdd.exe` via absolute
    `%SystemRoot%\System32\taskkill.exe` (mirror
    `uninstall.rs::system32_exe`), output discarded — best-effort like the
    Unix sweep. Unelevated taskkill only reaps same-user processes, matching
    `pgrep -U` intent. Forced kill is the accepted trade (§0.1: stop is
    signal-based on Unix; no IPC shutdown exists and adding one is a
    forbidden wire change) — Job Objects guarantee no orphaned children.
  - `start()`: `spawn_detached_windows(&self.yerdd_path)` —
    `Command::new(yerdd).arg("serve")`, null stdin/stdout/stderr,
    `.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP)`, map spawn
    error to `ServiceError::Spawn` (mirrors the Linux `spawn_detached`).
  - `restart()`: `self.stop()` → `wait_for_exit()` (50 × 100 ms poll of
    `yerdd_running()`) → `self.start()` — the Linux no-systemd shape,
    including the "must not start on `false`" timeout error.
  - `yerdd_running()`: absolute-path `tasklist.exe /FO CSV /NH /FI
    "IMAGENAME eq yerdd.exe"`, decided by pure
    `tasklist_lists(stdout: &str, exe: &str) -> bool` (a CSV row starting
    with `"yerdd.exe"`; the localized "no tasks" INFO line never contains the
    quoted image name, so this dodges the locale trap the old plan flagged
    for `sc query`). Cross-user matches only lengthen the wait loop — noted,
    accepted.
- **Login-entry API (new, `pub`, `#[cfg(windows)]` free functions)** — the
  enable/disable/status surface Unix keeps in the GUI, homed here for
  Windows so uninstall (S5), doctor (S4) and the Phase 6 GUI share one
  implementation:
  - `pub fn enable_at_login(yerdd: &Path) -> Result<(), ServiceError>` —
    open/create `HKCU\<RUN_KEY>` with `KEY_SET_VALUE`, `set_value(RUN_VALUE_NAME,
    &run_value_data(yerdd))`; then, if a `StartupApproved` value exists for
    the name, overwrite it with the 12-byte enabled pattern (§1.2).
  - `pub fn disable_at_login() -> Result<(), ServiceError>` — `delete_value`,
    `NotFound` tolerated as `Ok`.
  - `pub fn autostart_enabled() -> bool` — Run value present **and**
    `startup_approved_enabled(...)`; any registry error reads as `false`.
- **Pure helpers (ungated so their table tests run on every OS**, with
  `#[cfg_attr(not(windows), allow(dead_code))]` — the `parse_pids` pattern):
  `run_value_data`, `startup_approved_enabled`, `tasklist_lists`.
- Tests (in-file `mod tests`):
  - all-OS tables: `run_value_data` quotes the path and appends
    `serve --detach` (incl. a space-containing path); `startup_approved_enabled`
    over absent/enabled/disabled/short byte patterns;
    `tasklist_lists` over a CSV row, an `INFO:`-style line, empty output,
    and a different image name.
  - `#[cfg(windows)]` registry round-trip: parameterize the internals by key
    path + value name (thin `pub` wrappers pass the real constants) and
    round-trip write/read/delete against a scratch subkey
    `HKCU\Software\YerdTest-<pid>` — never the real Run key (the A7
    scratch-value precedent). Deleted in a drop guard.

**Compiles:** yes on all OSes (Unix arms untouched; new cfg(windows) code +
ungated pure fns).

### S2. `bin/yerdd` — `--detach` relauncher + logoff signal

- `src/args.rs`: `ServeArgs` gains
  `#[cfg(windows)] #[arg(long, hide = true)] pub detach: bool` — hidden,
  Windows-only, so the Unix CLI surface is byte-identical. Doc comment: the
  HKCU Run entry launches `serve --detach`; the process respawns itself
  hidden and exits so no console window persists at logon.
- `src/main.rs`: immediately after arg parsing (before tracing/runtime),
  `#[cfg(windows)] if args.detach { return relaunch_detached(&args); }` — a
  new `#[cfg(windows)] fn relaunch_detached(args: &ServeArgs) -> ExitCode`
  that spawns `current_exe()` with `serve` + the parsed `--verbose`/`--config`
  flags re-rendered (never raw argv, so `--detach` can't leak), null stdio,
  `creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP)`, prints a
  spawn failure to stderr → exit 70, else exit 0.
- `src/signals.rs`: add `ctrl_logoff()` as a fifth optional stream in the
  existing `#[cfg(windows)]` select (identical `recv_or_pending` pattern), so
  the autostarted daemon drains its Job Objects gracefully at logoff instead
  of taking the default-handler kill. Unix arms untouched.
- Unit test: a pure argv-rendering helper for `relaunch_detached`
  (`respawn_args(&ServeArgs) -> Vec<OsString>`) with a table test pinning
  that `--detach` is absent and config/verbose round-trip.

**Compiles:** yes. (Coordination: Track A's A6 also edits `main.rs` for
restart-spawn; land S2 after A6 or rebase trivially — the two touch disjoint
functions and A6's respawn helper may share the flag constants.)

### S3. Root `Cargo.toml` — `winreg` comment

The comment at `Cargo.toml:55-59` says "Read-only HKLM … never a write path".
5B introduces an HKCU **write** (Run value), as does Track A's A7 (HKCU
`Environment`). Whichever lands second just confirms; whichever lands first
rewrites the comment to: read HKLM (NRPT probe) + read/write **HKCU only**
(user-hive: PATH install, daemon Run entry) — still no HKLM writes outside
the helper. Trivial, no code.

### S4. Doctor — "autostart enabled" finding (the one contract touch)

- `crates/yerd-ipc/src/status.rs`: additive `DiagnosisCode` variant:
  ```rust
  /// The daemon is not registered to start at login, so sites stop being
  /// served after a reboot until it is started manually.
  DaemonAutostartDisabled,
  ```
  (snake_case wire tag `daemon_autostart_disabled`). Extend
  `tests/wire_stability.rs::diagnosis_code_each_variant_byte_shape` with the
  new pair. **Flag on review: IPC contract touch, additive-only.** Name is
  deliberately not A9's SCM-flavored `DaemonServiceNotRegistered` — there is
  no service to register.
- `crates/yerd-doctor/src/lib.rs`: `diagnose` gains
  `daemon_autostart_enabled: Option<bool>` (alongside `path_needs_setup`,
  and A9's `daemon_elevated` if that has landed — additive either way).
  `Some(false)` emits a `Warn`:
  title "Daemon autostart is off", detail "yerdd is not registered to start
  at login; sites won't be served after a reboot until it is started
  manually.", remedy `Some("turn on \"Start daemon at login\" in the Yerd
  app's settings")`. `None`/`Some(true)` emit nothing (existing convention,
  keeps macOS/Linux output byte-identical since their probe is `None`).
  Table tests for all three probe states.
- `bin/yerdd/src/ipc_server.rs`: new probe fn mirroring `path_needs_setup`
  (line 552):
  ```rust
  fn daemon_autostart_enabled() -> Option<bool> {
      #[cfg(windows)] { Some(yerd_service_ctl::autostart_enabled()) }
      #[cfg(not(windows))] { None }
  }
  ```
  passed at **both** diagnose call sites (`:235` and `run_doctor_fix`'s
  re-diagnose at `:964`).
- `bin/yerdd/Cargo.toml`: `[target.'cfg(windows)'.dependencies]
  yerd-service-ctl = { path = "../../crates/yerd-service-ctl" }`.
- "Daemon running": **already covered** — `DaemonDown` from
  `bin/yerd/src/lib.rs:918` fires on Windows when the pipe doesn't answer
  (§0.4). No change; its generic remedy ("start the daemon: yerdd") is valid
  on Windows.

**Compiles:** yes — the `diagnose` signature change and both call sites land
in this one step. (If Track A's A9 is mid-flight in the same tree, whoever
merges second adds their parameter beside the other's; both are additive.)

### S5. `bin/yerd/src/uninstall.rs` — remove the Run entry

In `windows_impl::run`, before `stop_daemon()` (mirroring the Unix order:
disable service → reap → delete unit):

```rust
match yerd_service_ctl::disable_at_login() {
    Ok(()) => println!("  removed the daemon autostart entry"),
    Err(e) => residue.push(format!("the 'Yerd Daemon' autostart entry may remain \
        under HKCU\\...\\CurrentVersion\\Run ({e})")),
}
```

Extend `print_header`'s daemon bullet to mention the login entry. No new dep
(`bin/yerd` already depends on `yerd-service-ctl`). The GUI's own `Yerd` Run
entry is the GUI/installer's to remove (Phase 6 uninstaller), exactly as Unix
uninstall leaves the GUI login item alone. Keep the local `stop_daemon`
taskkill as-is (5 lines; swapping it for `ServiceCtl::stop()` is optional
polish, not required).

**Compiles:** yes.

### S6. CLI — deliberate no-op (verify only)

Nothing to add (§0.5): no Unix-parity subcommand exists, so none is added; no
`ElevateTarget` variant, no helper op, no `HelperInvocation` change. The step
is a review assertion, not code: grep that 5B introduced no `elevate`/helper
surface.

---

## 3. Ordering constraints

- S1 → S4 (doctor probe calls the crate) and S1 → S5 (uninstall calls it).
- S2 is independent of S1 (the flag is inert until a Run entry exists) but
  must precede any real end-to-end login test; sequence S2 after Track A's A6
  (both edit `bin/yerdd/src/main.rs`).
- S4 should land **after** Track A's A9 if A9 is close, so `diagnose` grows
  its two probe params in one deliberate signature move per merge; otherwise
  S4 lands first and A9 adds beside it. Both orders compile; just don't
  interleave half-merged signatures.
- S3 coordinates with Track A's A7 (same comment line) — textual, trivial.
- Each step ends with a compiling workspace on all three OSes; the S4
  signature change and its two call sites are a single step by construction.

## 4. Tests (summary)

- **All-OS pure tables** (run on Linux/macOS CI too): `run_value_data`
  quoting, `startup_approved_enabled` byte patterns, `tasklist_lists` CSV
  parsing, `respawn_args` flag stripping, doctor probe-state table, wire-pin
  extension for `daemon_autostart_disabled`.
- **Windows CI leg**: yerd-service-ctl registry round-trip against a scratch
  HKCU subkey (never the real Run key); `autostart_enabled()` smoke
  (no-panic, `bool`); existing lifecycle/e2e suites unaffected.
- **Manual DoD checklist** (not CI): write the real entry via
  `enable_at_login` (temporary snippet or the Phase 6 GUI toggle), sign out /
  sign in → `yerd status` answers over the pipe, **no console window
  remains** (one brief flash accepted), Task Manager → Startup shows "Yerd
  Daemon"; disable in Task Manager → `yerd doctor` warns; logoff →
  clean teardown, zero orphaned php-cgi/DB processes; `yerd uninstall`
  removes the value (verify with `reg query`).

## 5. New pinned dependencies

**None.** `winreg 0.55` is already pinned in the workspace; S1/S4 add only
`[target.'cfg(windows)'.dependencies]` references. (`windows-service` and the
original plan's `ctrlc`-for-Track-B rationale are gone with the SCM design;
`ctrlc` remains solely a Track-A shim question, untouched here.)

## 6. Contract-boundary touches (flag on review)

1. **yerd-ipc (additive)**: one `DiagnosisCode` variant + wire-pin row (S4).
   Nothing else on the wire.
2. **Helper contract: untouched** — explicitly none, verified by S6.
3. **Pure-crate discipline**: decision logic is pure, table-tested, in-crate
   (`parse_pids` precedent); registry/process I/O stays at the crate's edge
   fns; `yerd-doctor` stays I/O-free (probe injected as `Option<bool>`).
4. **No sibling-repo contract changes** (consumer-side only).

## 7. Out of scope / handoffs (add to TODO.md)

- Phase 6 GUI wiring: `daemon_set_login`/`plan_start`/`daemon_stop`/
  `service_registered`/`is_set_up` Windows arms call
  `yerd_service_ctl::{enable_at_login, disable_at_login, autostart_enabled}`
  and `ServiceCtl::{start, stop}`; NSIS uninstaller removes both Run values.
- Phase 6 Windows self-update applier reuses `ServiceCtl` (restart path).
- Optional future: an additive IPC `Shutdown` request for a graceful
  CLI-driven stop (today's forced-kill mirrors Unix's signal model well
  enough; Job Objects make it safe).
- The logon console flash (§1.4) — documented wart; revisit only if users
  notice (conhost `--headless` is the escape hatch).

## 8. Definition of done

On Windows: enabling daemon autostart writes exactly
`HKCU\Software\Microsoft\Windows\CurrentVersion\Run` → `Yerd Daemon` =
`"<path>\yerdd.exe" serve --detach`; after sign-out/sign-in the daemon is
serving (pipe answers, sites load) with no persistent console; logoff/
shutdown drain cleanly with zero orphans; `yerd doctor` warns when the entry
is missing or Task-Manager-disabled and shows `daemon_down` when the daemon
is off; `yerd uninstall` leaves no Run value; all of it with **no unsafe, no
SCM, no UAC, no helper op, no new deps**; ubuntu/macos legs byte-identical
and green; the new pure tables demonstrably execute on all three CI legs.
