# Phase 5 Implementation Plan — Daemon lifecycle, autostart, CLI/PATH & PHP shims

Working doc (not committed). Companion to `WINDOWS_PLAN.md` Phase 5 and successor to
`PHASE4_PLAN.md` (Phases 1–4 committed as `ed9add2` / `1e0de5a` / `ed12af6` / `aa176b9`).
Everything below was verified against the actual code on this Windows machine on
2026-08-03, including an empirical compile probe of `windows-service 0.8.1` under
`#![forbid(unsafe_code)]` and a source read of its SCM API surface in the local cargo
registry.

Locked decisions honoured: **autostart = Windows Service (SCM)**, **shim delivery =
`.cmd` wrappers invoking `yerd.exe` in shim mode**. But see **§2 (D1)**: the
service-account/session-0 question cuts against the first lock hard enough that it
needs an explicit human confirmation before the service steps (Track B) are built.
Everything else (Track A) is decision-independent and can land first.

---

## 0. Ground truth (verified, not assumed)

### 0.1 Restart today

- `bin/yerdd/src/main.rs:93-106`: `restart_in_place()` is `#[cfg(unix)]` `exec()`;
  the non-Unix arm errors. `bin/yerdd/src/ipc_server.rs:241-252`: `RestartDaemon`
  is answered with an `Internal` error on non-Unix (`#[cfg(not(unix))]` arm).
- **By the time `main` sees `Outcome::Restart` the daemon has fully torn down**:
  `run_with_daemon` has returned, the tokio runtime is dropped (`main.rs:41`), so
  the IPC listener, the `fs4` instance lock (`single_instance.rs`), and every Job
  Object are already released before the re-exec. The Windows spawn-new path
  inherits this: the handoff race window is teardown-latency only, not
  overlapping-ownership.
- The pipe is **first-instance-exclusive**: `startup.rs`'s test
  `build_ipc_listener_binds_and_is_unique_per_dirs` proves a second listener on the
  same dirs fails. So "wait on the old pipe before binding" reduces to "bounded
  retry of normal startup" in the spawned child.
- The CLI already tolerates the handoff window: `bin/yerd/src/lib.rs:348-375`
  (`restart_and_await_boot_change`) sends `RestartDaemon` best-effort, then polls
  `Status.boot_id` for up to 15 s. The GUI (`commands.rs:437`, `tray.rs:840`) fires
  and forgets. **No client-side retry work is needed.**
- `std::os::windows::process::CommandExt::creation_flags` is safe std API - no FFI
  needed to spawn detached.

### 0.2 Signals today

- `bin/yerdd/src/signals.rs`: Unix selects `ctrl_c` + SIGTERM; non-Unix awaits only
  `tokio::signal::ctrl_c()`. All teardown (Job Object drain, pool/service stop) hangs
  off the one `watch::Sender<bool>` - any new signal source only needs to trip that.
- `bin/yerdd/Cargo.toml:53` already enables tokio's `signal` feature; the resolved
  tokio is **1.52.3** (Cargo.lock), whose `tokio::signal::windows` module provides
  `ctrl_c()`, `ctrl_break()`, `ctrl_close()`, `ctrl_shutdown()`, `ctrl_logoff()` -
  all safe wrappers over `SetConsoleCtrlHandler`. **No new crate, no FFI** (see §1.2).

### 0.3 The Phase 1 pipe and its DACL (cross-session reachability)

- Name: `yerd-<user SID>-<sha256(runtime dir)[..16]>` (`pure/win_pipe.rs::pipe_name`),
  derived identically by daemon (`startup.rs::build_ipc_listener`) and clients
  (`bin/yerd/src/transport.rs::exchange` → `yerd_platform::daemon_pipe_name`).
  `\\.\pipe\` names are global (no per-session namespace), so session hops are a
  DACL question only.
- SDDL: `D:P(A;;GA;;;SY)(A;;GA;;;<sid>)` (`pipe_sddl`), applied via `interprocess`'s
  `security_descriptor` and **proven applied** by the deny-probe test in
  `startup.rs` (`deny_sddl_is_applied_to_the_pipe`). So a SYSTEM-owned session-0
  server *can* create the pipe and the interactive user *can* connect - the Phase 1
  groundwork holds **provided the service derives the pipe name from the target
  user's SID and runtime dir**, not its own. That's the crux of D1 (§2): a
  LocalSystem `yerdd` calling today's `current_user_sid()` gets `S-1-5-18` and
  SYSTEM's `%TEMP%`, i.e. a *different pipe name entirely* - clients would never
  find it without explicit user-context plumbing.

### 0.4 Shim dispatch today (all `#[cfg(unix)]`)

- `bin/yerd/src/main.rs:11-30`: five `dispatch()` calls gated `#[cfg(unix)]`;
  `bin/yerd/src/lib.rs:13-31` gates the modules (`cli_shim`, `composer_shim`,
  `cover_shim`, `laravel_shim`, `wp_shim`, plus shared `shim`).
- Every shim keys off **`argv[0]`** (symlink name) and finishes with Unix
  `CommandExt::exec`. On Windows a `.cmd` wrapper cannot set argv[0], so dispatch
  needs an explicit sentinel argument (§3.A4).
- Portability holes found per module:
  - `shim.rs::cli_binary` builds `{data}/php/php-<minor>/bin/php`; the daemon's own
    Windows layout is `{data}/php/php-<minor>/php.exe`
    (`php_install.rs::cli_binary_path`, `#[cfg(windows)]` arm at line 708-711).
    The two are documented as kept byte-in-step - the shim side needs the same cfg
    split.
  - `shim.rs::default_from_shim` uses `read_link` on the `php` symlink -
    meaningless on Windows (wrappers aren't links); cfg-gate it out, the
    config-default → highest-installed fallbacks suffice.
  - `wp_shim.rs::site_scope` hardcodes `dirs.runtime.join("yerd.sock")` +
    `transport::exchange_at`; Windows needs `daemon_pipe_name(dirs)` +
    `transport::exchange_at_name` (both already exist, `transport.rs:54`).
  - `wp_shim.rs::quiet_deprecations_scan_dir_env` prefixes `:` (Unix
    `PHP_INI_SCAN_DIR` list separator); Windows PHP uses `;`.
  - `cover_shim.rs` hardcodes `pcov.so`; the daemon already installs **`pcov.dll`**
    on Windows (`ext_install.rs:74-82`, `PCOV_SPEC.so_name` cfg split, with a unit
    test pinning both names). The cover shim needs the same split (or better: reuse
    `ext_install`'s path is impossible cross-binary - mirror the constant like
    `cli_phprc` mirrors).
- Exit propagation: `exec` never returns on Unix; Windows spawn-and-wait must
  return the child's `status.code()`. On Windows `code()` is always `Some` (no
  signal-death), so mapping is total.

### 0.5 Shim installation today (all `#[cfg(unix)]`, symlink-based)

- `bin/yerdd/src/php_install.rs`: `place_symlink` (746), `set_default_shim`
  (768; non-Unix no-op at 778 returns the shim dir), `versioned_shim_name` (785,
  `php8.4` / `php8.4cover`), `managed_shim_version` (797, strict parse so the
  pruner never touches foreign files), `reconcile_shims` (833; non-Unix no-op at
  893). `shim_dir` = `{data}/bin` (736) is already ungated.
- `reconcile_shims` contract to preserve: one `discover_bundled` snapshot drives
  create+prune; callers hold the daemon's shim mutex; legacy versions get a clean
  shim but no cover shim; stale managed names are pruned by ownership.
- Tool shims: `bin/yerdd/src/tools/mod.rs::reconcile_tool_shims` (272; non-Unix
  no-op at 299) symlinks `composer`/`laravel`/`wp` → yerd multi-call, and
  `node`/`npm`/`npx`/`bun` → the real installed binaries (`node::shim_links`,
  `bun::shim_links`).
- Ownership rule for pruning on Unix is `is_symlink`. On Windows the equivalent
  must be "is a `.cmd` file whose content matches the yerd wrapper shape" - never
  delete a user's own `php.cmd` that yerd didn't write.

### 0.6 `yerd-service-ctl` today

- `crates/yerd-service-ctl/src/lib.rs`: `stop()` = launchctl-kill / systemctl-stop
  + pgrep/SIGTERM sweep; `start()` = kickstart / systemctl-start / detached spawn;
  `restart()` = kickstart -k / systemctl restart / stop-wait-start. The
  `not(macos|linux)` arms return `ServiceError::Unsupported`. Consumers today:
  `bin/yerd/src/apply.rs:314,490` (self-update applier) only.
- **Important for D1:** the existing Unix mechanisms are *per-user* managers -
  `launchctl gui/$uid/...` (a user LaunchAgent, macos.rs `service_target()`) and
  `systemctl --user`. Yerd has **no system-daemon precedent on any OS**; the
  Windows Service (session-0, boot-time) model in the lock would *exceed*, not
  match, the Unix behavior. Surfaced in §2.
- Style note: the crate deliberately shells out to platform tools. But `sc.exe`
  *query* output is localized text (locale-fragile parsing), and the daemon-side
  dispatcher can't be a subprocess at all - so Windows uses the `windows-service`
  crate rather than `sc.exe` for everything except one `sdset` call (§1.1).

### 0.7 PATH management today

- `bin/yerd/src/path_cmd.rs`: non-Unix `run` prints "not yet supported" and fails
  (line 21-32); non-Unix `ensure_installed_after_tool` is a no-op (47). All real
  logic is in the `unix` submodule (shell rc block editing).
- `winreg 0.55` is already a workspace dep (root Cargo.toml, "read-only" comment -
  that comment must be updated when Phase 5 adds the HKCU write path; HKCU is the
  user's own hive, so this does not cross a privilege boundary).
- The daemon's `path_needs_setup` probe (`ipc_server.rs:552`) returns `None` off
  Unix, so `BinDirNotOnPath` never fires on Windows today. Its Windows arm must
  read the **user's** `HKCU\Environment\Path` - trivially correct if the daemon
  runs as the user, another D1 casualty if it runs as SYSTEM.

### 0.8 Terminal, doctor, tunnel

- `WindowsTerminalLauncher` is an alias to the unsupported stub
  (`os/windows.rs:28-31`); sole call site is the GUI
  (`apps/yerd-gui/src-tauri/src/commands.rs:961-970`). The Linux impl's shape
  (probe list, spawn, first-success wins) is the model.
- `yerd-doctor` is pure: `diagnose(&StatusReport, path_needs_setup: Option<bool>)`.
  `DiagnosisCode` (yerd-ipc `status.rs:622`) is `#[non_exhaustive]` - additive
  variants are the sanctioned evolution path (wire-stability tests to extend).
  Port-conflict and resolver/cert findings already exist and are fed by the
  Windows `PortBinder`/`ResolverInstaller`/`TrustStore` probes from Phases 3-4;
  `resolver_remedy` already has a `#[cfg(windows)]` arm.
- Tunnel: `bin/yerdd/src/tunnel/install.rs:329-330` - `host_asset` returns `None`
  for Windows with an explicit "Phase 5" comment and a pinning test
  (`host_asset_is_none_for_windows`, line 569). Cloudflare publishes
  `cloudflared-windows-amd64.exe` (a bare exe, not gzipped; no arm64 asset).
  `yerd-tunnel`'s manager/supervision is OS-agnostic (Phase 2 Job Objects).

### 0.9 The elevated-helper contract (Phase 4) that service registration rides on

- `HelperInvocation` (`crates/yerd-platform/src/helper.rs`) + clap mirror
  (`bin/yerd-helper/src/cli.rs`) + debug wire cross-check; Windows launch via
  `runas` (`bin/yerd/src/elevate.rs:1073-1080`) with the **argv charset guard**:
  every argv element must be free of space/tab/quote/backslash (runas quoting
  bug). **Consequence: no Windows path may ever ride the elevated argv.** The
  register op therefore takes no binary path - the helper resolves `yerdd.exe` as
  its own sibling, exactly as `elevate.rs::sibling_binaries()` already does on the
  CLI side. A SID argument is charset-safe (`S-1-` + digits/dashes, already
  validated by `parse_whoami_sid`-style rules).
- Result-file protocol (`--result-token` + `helper_result`) and exit-code contract
  (0/65/77/78/-1) are reused verbatim.

---

## 1. FFI-touchpoint determinations (the security audit)

Verdict first: **Phase 5 requires zero `unsafe` in our code and zero new raw FFI.**
Every touchpoint lands on a safe crate, a safe std API, or a pinned-absolute-path
subprocess, consistent with the win32job/schannel/winreg/runas precedents.

### 1.1 Windows Service / SCM → `windows-service 0.8.1` (safe crate) — VERIFIED

Empirical probe compiled on this machine (scratch crate, `#![forbid(unsafe_code)]`,
exercising `define_windows_service!`, `service_dispatcher::start`,
`service_control_handler::register`, `set_service_status(SERVICE_RUNNING)`,
`ServiceManager::create_service`, `Service::query_status`, `Service::delete`):
**compiles clean under `forbid(unsafe_code)`** - the macro's internal `unsafe` is
inside the external-macro expansion, which the `unsafe_code` lint exempts, so the
workspace `forbid` is satisfied without any opt-out.

API coverage read from the crate source
(`~/.cargo/registry/src/.../windows-service-0.8.1/src/service.rs`):

| Need | Safe API | Win32 underneath |
|---|---|---|
| register | `ServiceManager::create_service` | `CreateServiceW` |
| unregister | `Service::delete` | `DeleteService` |
| reconfigure | `Service::change_config`, `set_description`, `set_delayed_auto_start` | `ChangeServiceConfigW`/`2W` |
| status query | `Service::query_status` (line 1539-1543) | **`QueryServiceStatusEx`** |
| start/stop | `Service::start` / `Service::stop` | `StartServiceW`/`ControlService` |
| dispatcher | `service_dispatcher::start` + `define_windows_service!` | `StartServiceCtrlDispatcherW` |
| STOP handling | `service_control_handler::register` closure | `RegisterServiceCtrlHandlerExW` |
| restart-on-exit | `update_failure_actions` + `set_failure_actions_on_non_crash_failures` | `ChangeServiceConfig2W` |
| per-user svc type | `ServiceType::USER_OWN_PROCESS` exists (service.rs:42) | (relevant to D1 option c) |

Dependency tree (verified via the probe's `cargo tree`): `bitflags 2`,
`widestring 1.2.1`, `windows-sys 0.61.2` (→ `windows-link 0.2.1`) - **every one
already resolves in yerd's Cargo.lock**. Net-new package: `windows-service` itself
only. Maintained by Mullvad, MIT/Apache.

**One gap:** the crate does not wrap `SetServiceObjectSecurity`, and by default
only admins may start/stop a service. To let the unelevated user (GUI,
self-update applier via `yerd-service-ctl`) control the daemon service without
UAC, the elevated helper runs one pinned-absolute-path subprocess at register
time: `%SystemRoot%\System32\sc.exe sdset yerdd <SDDL>` granting the target SID
`RP;WP;DT;LC` (start/stop/interrogate/query) on top of the stock SDDL. The SDDL
string is composed by a pure, table-tested helper (mirrors `pipe_sddl`). This is
the same subprocess pattern Phase 4 blessed for the helper (systemctl/nmcli
precedent, PHASE4_PLAN §0.1). → **no escalation needed for SCM itself.**

### 1.2 `SetConsoleCtrlHandler` → tokio's own signal module (no new crate)

`ctrlc` was investigated but is unnecessary for the daemon: tokio 1.52 (already a
dep with the `signal` feature enabled in `bin/yerdd`) exposes
`tokio::signal::windows::{ctrl_c, ctrl_break, ctrl_close, ctrl_shutdown}` - safe
streams over `SetConsoleCtrlHandler`, covering exactly the CTRL_C/BREAK/CLOSE set
the phase scope names (plus shutdown). Windows grants a ~5-10 s grace after
CTRL_CLOSE for the process to exit; the daemon's teardown (drop Job Objects,
pipe) is well inside that.

For the **shim** side (`bin/yerd`, no runtime in shim mode): the parent `yerd.exe`
must survive the console-broadcast Ctrl-C long enough to reap the child PHP and
propagate its exit code. `ctrlc = "3"` (safe crate: `SetConsoleCtrlHandler`
internally, unsafe inside the crate, none in ours; its default build handles
CTRL_C + CTRL_BREAK which is exactly what a console broadcast delivers) with a
no-op handler is the minimal correct tool. Determination: **adopt `ctrlc` for the
shim wait path only** (a ~3-line use); acceptable alternative if the human prefers
zero new deps: accept that Ctrl-C kills wrapper and child together (npm.cmd-style
behavior) and drop the dep. Recommended: adopt.

### 1.3 `WM_SETTINGCHANGE` broadcast → `setx.exe` side-channel (subprocess), not FFI

`SendMessageTimeoutW(HWND_BROADCAST, WM_SETTINGCHANGE, ..., "Environment", ...)`
has no safe-crate wrapper worth adopting. Without the broadcast, Explorer (and
anything it launches - every new Windows Terminal/PowerShell window) keeps its
stale `PATH` until logoff, so "resolve from a fresh shell" would silently fail -
skipping is *not* cosmetic. Determination: after the winreg write, run
`%SystemRoot%\System32\setx.exe YERD_BIN <shim-dir>` - `setx` writes an
(independently useful) HKCU env var **and always broadcasts `WM_SETTINGCHANGE`**
as documented side effect. `PATH` itself is never written through `setx` (its
1024-char truncation bug), only through `winreg` read-modify-write. No unsafe, no
new dep. Fallback if the human dislikes the marker variable: skip the broadcast,
print "open a new terminal after logging off/on", tracked TODO. Recommended: setx.

### 1.4 Detached spawn / new console → safe std

`std::os::windows::process::CommandExt::creation_flags` covers both the restart
respawn (`CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW`) and the terminal fallback
(`CREATE_NEW_CONSOLE` for `powershell.exe`/`cmd.exe`). Safe std, no dep.

### 1.5 HKCU `Environment` writes → `winreg` (already a dep)

Write path added to the existing read-only usage; update the root Cargo.toml
comment beside `winreg` accordingly. `RegValue`-level read/write preserves
`REG_EXPAND_SZ` vs `REG_SZ` (read the raw value type, write the same type back;
create as `REG_EXPAND_SZ` when absent). The list edit itself is pure and
table-tested (§3.A1). HKCU is the invoking user's own hive - no privilege
boundary crossed, same trust level as editing `~/.zshrc` on Unix.

---

## 2. ESCALATION / DECISIONS — read before building Track B

### D1 (BLOCKING for Track B): the service account / session-0 data-dir model

The locked decision says "Windows Service (SCM), starts at boot, survives
logoff". The problem: **`yerdd`'s entire state model is per-user.** Phase 1's
`WindowsPaths` resolves `%APPDATA%\yerd` / `%LOCALAPPDATA%\yerd` / `%TEMP%\yerd`
from the *process's own* environment; the pipe name embeds the *process's own*
SID + runtime dir (§0.3); the daemon probes the **CurrentUser** cert store for
`StatusReport.ca.trusted_system` and the PHP CA bundle (`os/windows.rs`
`WindowsTrustStore`, `system_root_bundle`); `path_needs_setup` will read HKCU.
A service account that isn't the user gets the wrong answer to *all* of these.

Also verified (§0.6): on macOS/Linux the daemon is a **per-user login-scoped
agent** (`launchctl gui/$uid`, `systemctl --user`), not a system daemon. The
lock's premise ("matching the systemd-system / launchd-daemon model") does not
match the shipped Unix behavior - a session-0 boot service would be *more*
privileged/global than Yerd is anywhere else.

Options:

**(a) Service runs as the interactive user account.**
`ServiceInfo.account_name`/`account_password` support this, BUT: `CreateService`
needs the user's *password* (collecting it is unacceptable UX and a handling
liability), the account needs `SeServiceLogonRight` (an LSA policy write that
neither `windows-service` nor `sc.exe` can grant - raw `LsaAddAccountRights` FFI
or `secedit` gymnastics), and a later password change bricks the service.
**Non-starter. Rejected.**

**(b) LocalSystem service + explicit target-user context.**
Register with launch args carrying the target user (SID is runas-argv-safe) and
plumb explicit dirs through `bring_up` (the user's `%APPDATA%`/`%LOCALAPPDATA%`
paths resolved at install time; pipe name derived from the *target* SID +
*target* runtime dir - §0.3). Pros: boot start, survives logoff, honours the
lock verbatim; the Phase 1 pipe SDDL (`(A;;GA;;;SY)`) was designed for exactly
this. Cons, and they are serious:
  - **Local privilege escalation by design**: SYSTEM would execute
    `php-cgi.exe`, `mysqld.exe`, `cloudflared.exe` etc. out of user-writable
    `%LOCALAPPDATA%\yerd\data` - any code running as the user swaps a binary and
    gets SYSTEM. On Unix the daemon runs *as the user*, so Yerd has never had
    this exposure. Mitigating (ACL-locking the data tree to SYSTEM) breaks the
    rootless user-owned-state design outright.
  - All files the daemon writes (config mutations, site state, logs, PHP ini)
    become SYSTEM-owned inside the user's profile - the user's own CLI/GUI and
    uninstaller then fight ACLs.
  - CurrentUser-store trust probes and HKCU PATH probe still resolve the
    *service's* hive; each needs a per-probe rework or an honest `None`.
  - Estimated extra work over (d): explicit-dirs/SID plumbing through
    `startup.rs` + `Paths`, ACL handling, probe rework - roughly doubles Track B.

**(c) Per-user service (Windows user-service template).**
`ServiceType::USER_OWN_PROCESS` is expressible in `windows-service` (§1.1), and
instances run *as the user in the user's session* - all path/probe/pipe problems
vanish. But: the template requires extra raw registry flags
(`UserServiceFlags`) under the service key, instances are named
`yerdd_<hex>` per logon, Microsoft documents the mechanism primarily for inbox
services, and - decisive - user services start at **logon** and stop at
**logoff**, so it does not deliver "boot + survives logoff" either. It is a
more exotic spelling of (d) with SCM branding.

**(d) Reconsider: per-user autostart at logon (revisit the lock).**
`HKCU\...\Run` entry (zero elevation - the same mechanism the GUI's
tauri-plugin-autostart already uses) or a per-user logon Task Scheduler task,
spawning `yerdd serve` detached. Matches the launchd-gui-agent /
`systemctl --user` semantics Yerd actually ships on Unix; keeps every Phase 1-4
assumption intact (paths, SID, pipe name, cert probes, HKCU); registration
needs **no helper op, no UAC, no new contract**; `yerd-service-ctl` Windows
backend becomes: Run-key write + `taskkill`/spawn + (status via pipe ping or
process query). Cost: the daemon starts at first logon, not boot, and stops at
logoff - for a *local dev* tool serving the logged-in user's sites, nothing is
lost in practice (there is nobody to serve before logon).

**RECOMMENDATION: (d)** - ask the user to confirm downgrading the "Windows
Service" lock to a per-user logon autostart, on the grounds that (i) it matches
the per-user daemon model Yerd ships on macOS/Linux (the lock's stated premise
was inaccurate), and (ii) (b) introduces a genuine SYSTEM privilege-escalation
path that contradicts the project's rootless security posture. **If the human
reaffirms the lock**, build (b) with: explicit `--serve-sid`/dirs args baked at
registration, `sc.exe sdset` user grant (§1.1), CurrentUser-store probes forced
to `None` under a non-user account (honest doctor), and a documented LPE caveat
in SECURITY.md. Track B below is written for the SCM shape and marks the (b)-only
sub-steps; under (d), Track B steps B2/B3 shrink and B1 (helper op) is dropped.

### D2 (minor): `ctrlc` dep for shim Ctrl-C swallowing — recommended yes (§1.2)

### D3 (minor): `setx` marker-variable broadcast vs skip-with-TODO — recommended setx (§1.3)

No other escalation: no `unsafe`, no raw FFI anywhere in Phase 5 (§1).

---

## 3. File-by-file implementation checklist (ordered; workspace compiles after every step)

### Track A — decision-independent (start immediately)

**A0. Machine verification spike (no commit)** - done during planning:
`windows-service` forbid-probe (§1.1) ✔; crate API audit ✔; tokio signal
availability ✔. Remaining at implementation start: confirm `setx.exe` broadcast
refreshes a fresh Windows Terminal's PATH on this machine; confirm `wt.exe -d`
launch shape.

**A1. `yerd-platform` pure helpers** (compiles + table-tests on all OSes):
- `crates/yerd-platform/src/pure/win_shim.rs` (new): `wrapper_body(yerd_exe: &Path,
  shim_name: &str) -> String` rendering exactly (CRLF line endings - cmd.exe
  mishandles bare-LF batch files):
  ```
  @echo off\r\n
  "<abs-yerd-exe>" __shim <shim-name> %*\r\n
  exit /b %ERRORLEVEL%\r\n
  ```
  plus `is_yerd_wrapper(content: &str) -> bool` (ownership probe for pruning: the
  `__shim` invocation line is the marker) and `wrapper_file_name(shim: &str) ->
  String` (`<shim>.cmd`). Golden tests incl. spaces in the exe path.
- `crates/yerd-platform/src/pure/win_path_env.rs` (new): `upsert_entries(current:
  &str, add: &[&str]) -> Option<String>` / `remove_entries(...)` - semicolon
  split, case-insensitive + trailing-slash-insensitive compare, order-preserving
  append, `None` when unchanged. Table tests: empty value, absent trailing `;`,
  pre-existing long PATH, duplicate casing.
- (Track B, but pure) `pure/win_svc.rs`: `service_sddl_with_user(sid) -> String`
  for the `sc.exe sdset` grant; golden test. Register in `pure/mod.rs`.

**A2. `yerd-platform`: real `WindowsTerminalLauncher`** (`os/windows.rs`; remove
the alias from the `pub use super::unsupported` list in the same change - the
never-half-flip rule):
fallback chain `wt.exe -d <path>` → `powershell.exe` (`-NoExit`,
`creation_flags(CREATE_NEW_CONSOLE)`, `current_dir`) → `cmd.exe` (same flags).
PATH lookup for `wt.exe` is fine (it's an app-execution alias; terminal launch is
not a security surface - Linux impl already PATH-probes). Unit-test the pure
command-shape helper; the actual spawn is exercised manually via the GUI.

**A3. `bin/yerdd`: Windows shutdown signals** (`signals.rs`):
in the `#[cfg(not(unix))]` arm (make it `#[cfg(windows)]` + keep a bare-ctrl_c
`#[cfg(not(any(unix, windows)))]` arm), `tokio::select!` over
`ctrl_c()`, `ctrl_break()`, `ctrl_close()`, `ctrl_shutdown()` **plus** a
`pub(crate) static SERVICE_STOP: tokio::sync::Notify` (new, in `signals.rs`) so
the Track-B SCM handler can trip the same watch. Each stream's install failure
degrades to the remaining streams (mirror the Unix SIGTERM-failure fallback).
Teardown past the watch is unchanged - Phase 2 Job Objects drain exactly as on
Ctrl-C today. Test: lifecycle already covers watch-tripped teardown; add a unit
test that `SERVICE_STOP.notify_waiters()` resolves `wait_for_shutdown`.

**A4. `bin/yerd`: shim dispatch un-gated (spawn-and-wait)**:
- `shim.rs`: un-gate module (drop `#[cfg(unix)]` in lib.rs). Add
  `shim_invocation() -> Option<(String, Vec<OsString>)>`: argv[0] basename
  (existing behavior, all OSes - Unix symlinks keep working) **or** the sentinel
  `argv[1] == "__shim"` → `(argv[2], argv[3..])` (all OSes, so Unix tests can
  drive it too). `__shim` is claimed before clap parses; a stray user-typed
  `yerd __shim` with no name prints a usage error (exit 2).
  Add `run_php(cmd: Command) -> ExitCode`: `#[cfg(unix)]` `exec()` (today's
  code), `#[cfg(windows)]` install `ctrlc` no-op handler (D2), `status()`,
  propagate `code()`. Fix `cli_binary` (`php.exe` flat layout, §0.4) and cfg-gate
  `default_from_shim`.
- `cli_shim.rs` / `composer_shim.rs` / `laravel_shim.rs` / `cover_shim.rs` /
  `wp_shim.rs`: replace `dispatch()`'s argv[0] read with `shim_invocation()`,
  route the final `exec` through `run_php`, un-gate. Per-module fixes from §0.4:
  wp `site_scope` transport cfg split (pipe vs socket), `PHP_INI_SCAN_DIR`
  separator cfg (`;` prefix on Windows), cover `pcov.dll` cfg const.
- `main.rs` / `lib.rs`: drop the five `#[cfg(unix)]` gates; `Command::Coverage`'s
  non-Unix refusal arm (lib.rs:50-61) becomes the real `run_coverage` call.
- Tests: existing per-module unit tests now compile on Windows; add
  `shim_invocation` table tests (sentinel + argv0 + precedence).

**A5. `bin/yerdd`: `.cmd` wrapper delivery** (`php_install.rs`, `tools/mod.rs`):
- `php_install.rs`: `versioned_shim_name` gains `#[cfg(windows)]` twin appending
  `.cmd` (or one ungated fn + cfg suffix const); `managed_shim_version` strips a
  `.cmd` suffix before the existing strict parse (ungate it).
  New `#[cfg(windows)] place_wrapper(path, yerd_bin, shim_name)`: write via temp
  sibling + rename (mirror `place_symlink`'s atomicity), **skip the write when
  existing content already matches** (idempotent, AV-friendly). Windows
  `set_default_shim` writes `php.cmd`; Windows `reconcile_shims` mirrors the Unix
  algorithm 1:1 (same snapshot/prune contract, §0.5) with
  `is_yerd_wrapper`-gated pruning instead of `is_symlink`.
- `tools/mod.rs`: Windows `reconcile_tool_shims` covering the yerd-multicall
  tools only - `composer.cmd`, `laravel.cmd`, `wp.cmd` (Node/Bun expose real
  foreign binaries whose Windows install story isn't wired; explicit TODO + skip,
  §8). Prune by wrapper-ownership.
- The absolute `yerd_bin` embedded in wrappers is the path the daemon already
  passes to `reconcile_shims`; Phase 6's staged-rename self-update keeps that
  path stable, so wrappers survive updates unmodified (master-plan invariant).
- Tests (Windows CI leg): port the three `reconcile_*` tests to Windows
  (`#[cfg]` fixtures write wrapper files instead of symlinks); golden wrapper
  content; foreign `php.cmd` left untouched.

**A6. Restart: spawn-new-then-exit** (`bin/yerdd/src/main.rs`,
`ipc_server.rs`, `startup.rs`):
- `ipc_server.rs`: collapse the two `RestartDaemon` arms to the accept path on
  Windows too (keep refusal for `not(any(unix, windows))`).
- `main.rs` `#[cfg(windows)] restart_in_place()` → `restart_spawn()`: after the
  existing full teardown (§0.1), spawn `current_exe()` with the original argv,
  env `YERD_RESTART_HANDOFF=1`,
  `creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW)`, then return
  success (docs note: a foreground console daemon restarts into the background;
  logs continue in `{cache}/`). Update the now-stale "unreachable on non-Unix"
  doc comments.
- `startup.rs` (or `single_instance.rs`): when `YERD_RESTART_HANDOFF` is set
  (consume it so it doesn't leak to grandchildren), wrap `InstanceLock::acquire`
  + `build_ipc_listener` in a bounded retry (~40 × 250 ms) - the pipe's
  first-instance exclusivity (§0.3) makes "old pipe gone" exactly equal to "bind
  succeeds", which is the readiness signal the master plan asks for. No env var →
  today's fail-fast `AlreadyRunning` behavior is untouched.
- Test (Windows CI leg, new `bin/yerdd/tests/restart_windows.rs`): spawn the
  built `yerdd.exe` (`CARGO_BIN_EXE_yerdd`) with `APPDATA`/`LOCALAPPDATA`/`TEMP`
  pointed at a tempdir, ping over the derived pipe, capture `boot_id`, send
  `RestartDaemon`, poll until `boot_id` changes, then reap the process tree
  (taskkill on the child pid) in a `Drop` guard.

**A7. CLI install / PATH** (`bin/yerd/src/path_cmd.rs`, dep: A1):
- New `#[cfg(windows)] mod windows` replacing the current stub `run`:
  - `Install`: copy `current_exe()` → `%LOCALAPPDATA%\Programs\yerd\bin\yerd.exe`
    (create dirs; skip copy when source == dest or contents already current;
    tolerate `ERROR_SHARING_VIOLATION` by `.new`-staging beside it with a note -
    full staged-swap semantics stay Phase 6); winreg `HKCU\Environment` `Path`
    read-modify-write via `win_path_env::upsert_entries` with the two entries
    (`...\Programs\yerd\bin`, `{data}\bin`), preserving the value type (§1.5);
    broadcast via `setx YERD_BIN <shim-dir>` (D3).
  - `Uninstall`: `remove_entries` for both dirs (+ best-effort delete of the
    `Programs\yerd` copy, tolerated-if-locked); leave `YERD_BIN` removal to it too.
  - `Print`: print the two dirs and a `setx`-free manual snippet.
  - `ensure_installed_after_tool`: same upsert, quiet.
- `bin/yerd/Cargo.toml`: add `[target.'cfg(windows)'.dependencies] winreg`,
  `ctrlc` (D2). Update the root `winreg` comment (read-only → +HKCU Environment
  write, §0.7).
- `uninstall.rs` windows_impl: call the PATH removal before deleting dirs
  (today's residue note about binaries stays; extend `dirs_to_delete` note to
  mention `Programs\yerd`).
- Tests: pure edits are covered in A1; add an ignored-by-default manual test or a
  registry round-trip test against a scratch value name (NOT `Path`) under
  `HKCU\Environment` - never mutate the real `Path` in CI.

**A8. Tunnel** (`bin/yerdd/src/tunnel/install.rs`):
`host_asset` Windows arm → `("cloudflared-windows-amd64.exe", false)` for
`X86_64`, `None` for arm64 (Cloudflare publishes none - keep the honest error);
binary name `cloudflared.exe` on disk; flip the
`host_asset_is_none_for_windows` test to pin the new mapping. Supervision/login
paths are already OS-neutral (Phase 2). e2e is a manual DoD item (needs a real
Cloudflare login).

**A9. Doctor (decision-independent parts)** (`yerd-doctor`, `yerd-ipc`,
`ipc_server.rs`):
- `yerd-ipc`: **ADDITIVE** `DiagnosisCode` variants `DaemonServiceNotRegistered`
  (autostart not set up; remedy text per-OS) and `DaemonElevated` (the daemon is
  running with an elevated/SYSTEM token - it should run as the user); extend the
  wire-stability pins. Flagged: this is an IPC-contract touch, additive-only.
- `yerd-doctor::diagnose` gains two probe params alongside `path_needs_setup`
  (`service_registered: Option<bool>`, `daemon_elevated: Option<bool>`; `None`
  emits nothing - existing convention). Table tests.
- `ipc_server.rs`: supply the probes - `daemon_elevated` =
  `yerd_platform::is_token_elevated()` on Windows / `None` elsewhere;
  `service_registered` wired in Track B (pass `None` until then); Windows
  `path_needs_setup` arm = any tool installed && shim dir missing from
  `HKCU\Environment\Path` (via `win_path_env` + winreg read; `yerdd` gains the
  same `cfg(windows)` winreg dep). Port-80/443-reservation and NRPT/cert findings
  already exist (§0.8) - no new work.

**A10. Port the shim e2e tests** (`bin/yerd/tests/`):
- `cli_e2e.rs`, `wp_shim_e2e.rs`: replace the `#[cfg(unix)]` mod gate with
  cfg-split transport helpers exactly as `bin/yerdd/tests/lifecycle.rs` already
  does (`exchange_at` vs `exchange_at_name(daemon_pipe_name(dirs))`); fixture
  fixes: `fake_php_cli` writes `php.exe` at the Windows layout.
- `cover_shim_e2e.rs`: the stub interpreter is a shell script - on Windows,
  compile a 15-line stub (prints `phprc=%PHPRC%`/args, re-execs itself once) with
  `rustc -O -o php.exe stub.rs` at test start (rustc is definitionally present on
  the CI leg); assert PHPRC inheritance + arg forwarding + exit-code propagation
  through the real `yerd.exe __shim php8.4cover` invocation and a real generated
  `.cmd` wrapper (this is the e2e guard for the "cmd hop must not swallow the
  exit code" risk).

### Track B — Windows Service / autostart (BLOCKED on D1; written for the SCM shape)

**B1. Helper contract: register/unregister ops** (`yerd-platform/src/helper.rs`,
`bin/yerd-helper`) - **ADDITIVE helper-contract change, flag on review**:
- `HelperInvocation::RegisterDaemonService { sid: String }` /
  `UnregisterDaemonService` (argv `register-daemon-service --sid S-1-...` /
  `unregister-daemon-service`); `from_argv`/`to_argv` arms + round-trip tests +
  the runas charset guard test (SID-only argv is safe by construction, §0.9). No
  path argument ever (runas backslash rule): the helper resolves `yerdd.exe` as
  its own sibling and refuses if missing (mirror `Setcap`'s basename validation).
- `bin/yerd-helper/src/ops/service.rs` (new, `#[cfg(windows)]`): register =
  `create_service` (auto-start; under D1(b): launch args `serve --service
  --serve-sid <sid>` (+ explicit dirs args); delayed-auto-start true;
  description) + `update_failure_actions` (restart-on-failure, 1 s delay) +
  `set_failure_actions_on_non_crash_failures(true)` + `sc.exe sdset` user grant
  (§1.1, absolute path, composed from `pure::win_svc`); idempotent (open-then-
  `change_config` when it already exists). Unregister = stop-if-running, delete,
  tolerate `ERROR_SERVICE_DOES_NOT_EXIST`. Non-Windows arms: `Unsupported`
  (mirror `Setcap`'s Linux-only pattern). Wire into `cli.rs`/`exec.rs` dispatch.
- Deps: `bin/yerd-helper` + `crates/yerd-service-ctl` gain
  `[target.'cfg(windows)'.dependencies] windows-service` (see B2 - the SCM calls
  live in `yerd-service-ctl`, the helper just calls them; keeps one SCM surface).

**B2. `yerd-service-ctl`: Windows backend**:
- `#[cfg(windows)]` module: `SERVICE_NAME = "yerdd"`; `register(sid)` /
  `unregister()` (used by the helper, elevated); `start()` / `stop()` /
  `restart()` via `windows-service` `Service::start/stop` + bounded
  `query_status` wait (replacing the `Unsupported` arms in `ServiceCtl`;
  unelevated thanks to the `sdset` grant); fallback when the service isn't
  registered: `taskkill /IM yerdd.exe` by absolute `System32` path + detached
  spawn (mirrors the Linux no-systemd arm; Job Objects reap children). New
  `pub fn daemon_service_status() -> Option<DaemonServiceStatus>`
  (`NotRegistered` / `Registered { running: bool }`) via
  `open_service(QUERY_STATUS)` - the SCM-query replacement for pgrep, consumed by
  doctor (B4) and the GUI later (Phase 6).
- Tests: pure name/SDDL pins; SCM integration is admin-gated → `#[ignore]`d
  manual test + the DoD checklist.

**B3. `bin/yerdd`: service-mode entry point** (new `service_mode.rs`, `args.rs`,
`main.rs`):
- `ServeArgs` gains a hidden `#[cfg(windows)] --service` flag. `main`: when set,
  call `service_mode::run()` → `service_dispatcher::start("yerdd",
  ffi_service_main)`; `define_windows_service!(ffi_service_main, service_main)`.
- `service_main`: register the control handler (Stop/Shutdown →
  `signals::SERVICE_STOP.notify_waiters()` (A3) + `NoError`; Interrogate →
  `NoError`); report `StartPending` → build runtime → `Running`
  (`ServiceControlAccept::STOP`); run the normal `yerdd::run(args)`;
  on the way out `StopPending` (wait-hint ≥ teardown budget) → `Stopped` with
  exit code 0 for `Outcome::Exit`, `ServiceExitCode::ServiceSpecific(1)` for
  `Outcome::Restart` - the failure-actions config from B1 makes the SCM itself
  respawn the service, which *is* the service-mode restart (the A6 spawn-new path
  stays console-mode-only; `restart_spawn` guards on "not running as service").
  Console `cargo run -p yerdd` / `yerdd serve` is untouched (no dispatcher).
- Under D1(b) only: `--serve-sid`/dirs overrides plumb the target user's
  `PlatformDirs` + pipe SID through `bring_up`/`build_ipc_listener` (this is the
  expensive part §2 prices in; under D1(d) this whole bullet disappears and B3
  reduces to nothing - no service mode at all).
- Test: dispatcher paths need a real SCM registration → manual DoD item;
  unit-test the handler→Notify wiring (A3 test already covers the watch side).

**B4. Wire-up: elevate / uninstall / doctor / GUI hook point**:
- `bin/yerd/src/cli.rs`: `ElevateTarget::Service` variant (doc: "register the
  daemon to start automatically"); Unix `run_one` arm = explicit skip-with-note
  (mirror how Ports/Lan skip on Windows); Windows arm spawns the helper with
  `RegisterDaemonService { sid: current_user_sid()? }` (+ `--result-token`),
  classifies exit codes identically to the resolver arm; `unelevate service` →
  `UnregisterDaemonService`. No IPC involvement (facts needed: none - the helper
  self-locates yerdd).
- `uninstall.rs` windows_impl: before `stop_daemon()`, run the helper
  `UnregisterDaemonService` (one more UAC prompt, mirroring `revert_nrpt`;
  residue note on failure).
- Doctor: `ipc_server.rs` feeds `service_registered` from
  `yerd_service_ctl::daemon_service_status()` (A9 already added the plumbing;
  `Some(false)` → `DaemonServiceNotRegistered` warn with remedy
  `yerd elevate service`). `bin/yerdd` gains the `yerd-service-ctl` dep
  (lib←bin, downhill, allowed).
- GUI autostart toggle wiring stays Phase 6 (master plan; `autostart.rs`'s
  `not(linux|macos)` arms already compile as no-ops).

**B5. CI / full gate**: `cargo fmt`/`clippy -D warnings`/`test` on all three
OSes; verify the newly un-gated tests actually *ran* on the Windows leg (grep the
test list - the vacuous-green rule); `npm test`/`build` untouched (no frontend
changes in Phase 5).

---

## 4. Ordering constraints

- A1 → A5/A7 (pure helpers first). A3 → B3 (Notify exists before the SCM handler).
  A4 → A5 → A10 (dispatch before delivery before e2e). A6 independent after A3.
  A9 → B4 (doctor plumbing before the service probe fills it). B1 → B2 consumers →
  B3/B4. Track A has no dependency on D1 and should land while D1 is decided.
- Never-half-flip: A2 replaces the `WindowsTerminalLauncher` alias and adds the
  impl in the same change. Same rule if B-work touches `os/windows.rs`.
- Each lettered step ends with a compiling workspace on all three OSes (the
  un-gating steps A4/A5 must land module + call sites together).

## 5. Tests to add / port (summary)

Pure (all OSes): `win_shim` wrapper golden + ownership probe; `win_path_env`
tables; `win_svc` SDDL golden; `shim_invocation` tables; doctor probe tables;
helper argv round-trip + runas-charset guard for the new ops.
Windows CI leg: ported `cli_e2e.rs` / `wp_shim_e2e.rs` / `cover_shim_e2e.rs`
(rustc-compiled `php.exe` stub; exit-code + PHPRC + `%*` forwarding through a real
`.cmd`); `reconcile_shims`/`set_default_shim`/tool-shim Windows fixtures;
`restart_windows.rs` spawned-process boot-id test; `SERVICE_STOP` notify test.
Manual/admin-gated (DoD checklist, not CI): SCM register→boot→logoff survival →
GUI reachability over the pipe; `sc sdset` grant lets the unelevated user
stop/start; NRPT+cert+shim doctor all-green; cloudflared e2e; `setx` broadcast
freshness.

## 6. New pinned dependencies (complete list)

| Crate | Where | Verdict |
|---|---|---|
| `windows-service = "0.8"` | workspace pin; `cfg(windows)` dep of `yerd-service-ctl`, `bin/yerdd`, (`bin/yerd-helper` if it calls SCM directly rather than via yerd-service-ctl) | Safe API over SCM (§1.1, empirically verified under `forbid(unsafe_code)`); tree already fully in Cargo.lock except the crate itself. Pin comment per house style. **Track B only.** |
| `ctrlc = "3"` | `cfg(windows)` dep of `bin/yerd` | D2; safe crate, shim Ctrl-C swallow only. Optional. |
| (none else) | | `winreg`, `widestring`, `runas`, tokio-signal already present. `setx`/`sc.exe`/`taskkill` are absolute-path subprocesses, not deps. |

## 7. Contract-boundary touches (flag on review)

1. **yerd-ipc (additive)**: two new `DiagnosisCode` variants (A9); wire-stability
   pins extended. `RestartDaemon` un-gating is behavior-only, no wire change.
2. **Helper contract (additive)**: `RegisterDaemonService { sid }` /
   `UnregisterDaemonService` variants + argv shape + debug cross-check (B1) -
   only under D1(b)/SCM; D1(d) needs **no** helper change.
3. **Pure-crate discipline**: all new decision logic (wrapper bodies, PATH edits,
   service SDDL, shim-name parsing) lands in `yerd-platform/src/pure/` with
   table tests; I/O stays in the binaries/os-modules.
4. **No sibling-repo contract changes**: shims/service/PATH are all
   consumer-side. `cloudflared-windows-amd64.exe` is Cloudflare's published name,
   not a yerd-* contract.

## 8. Explicitly out of scope / TODO handoffs (add to TODO.md)

- Node/Bun tool shims on Windows (their Windows install pipeline isn't wired;
  `reconcile_tool_shims` skips them with a comment) - post-MVP.
- GUI wiring: autostart toggle → service ops, `cli_path_status` Windows arm,
  Windows copy audit - Phase 6 per master plan.
- NSIS install hooks invoking register/PATH ops - Phase 6.
- `SystemMetrics` via `GetProcessMemoryInfo` - tracked TODO (master plan).
- `.cmd` wrapper "Terminate batch job (Y/N)?" prompt on Ctrl-C - known cmd.exe
  wart (npm has it); document, don't solve.
- If D3 lands as "skip broadcast": TODO for the `SendMessageTimeoutW` story.

## 9. Definition of done (master plan, made concrete)

On Windows: fresh shell resolves `yerd`, `php`, `php8.4`, `composer`, `wp`,
`laravel`, `phpcover` (via `.cmd` wrappers + HKCU PATH); `php -v` inside a
registered site dir runs that site's pinned version (wp path proven by e2e);
exit codes and `PHPRC` survive the cmd hop (e2e-pinned); `yerd restart` completes
with a changed `boot_id`; Ctrl-C/console-close/service-stop all tear down with
zero orphaned processes; autostart per the D1 outcome is registered, survives the
D1-appropriate lifecycle, and the per-user GUI reaches the daemon over the Phase 1
pipe; `yerd doctor` reports the Windows checklist (service/autostart, elevation,
ports, NRPT, cert, PATH+shims); ported shim e2e + restart tests demonstrably
execute on the Windows CI leg; ubuntu/macos byte-identical and green.
