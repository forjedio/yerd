# Windows Support — Phased Implementation Plan

Working doc (not committed). Target: functional parity with macOS/Linux features on
Windows, MVP simplifications allowed where noted. macOS/Linux must keep compiling and
passing tests after every phase.

**Decisions locked by the user (2026-08-02):**

1. **Cert store:** CurrentUser Root store for the Yerd CA (Phase 3).
2. **`.test` DNS:** **NRPT wildcard** (`Set-DnsClientNrptRule`, true `*.test`) from the
   start — NOT the hosts file (Phase 4).
3. **Daemon autostart:** **per-user logon autostart** (HKCU Run key, the same
   mechanism the GUI already uses via tauri-plugin-autostart). REVERSED from the
   earlier "Windows Service (SCM)" choice during Phase 5 after review found a real
   Service to be broken and a security risk here: a session-0 LocalSystem daemon
   resolves the wrong per-user profile (`%LOCALAPPDATA%` → systemprofile), derives a
   non-matching IPC pipe name (pipe name embeds the process SID, so per-user clients
   cannot connect), and is a local privilege escalation (SYSTEM launches
   user-writable binaries from `%LOCALAPPDATA%\yerd`). Yerd's actual Unix autostart
   is per-user too (`launchctl gui/$uid`, `systemctl --user`), so per-user logon
   autostart is the correct parity, needs no admin/UAC, and keeps every Phase 1-4
   assumption intact. This removes the Phase-1 cross-session-pipe requirement's
   rationale (the daemon and clients now share the user's SID), though the
   deterministic SID-keyed pipe + DACL from Phase 1 stays correct and unchanged.
4. **Shim delivery:** **`.cmd`/`.bat` wrappers** invoking `yerd.exe` in shim mode —
   NOT exe-copies, NOT symlinks (Phase 5).
5. **Packaging/signing (Phase 6):** still open; decide at Phase 6 planning.

Guiding principles:

- Almost all work is **additive** in `yerd-platform` (new `os/windows.rs` impls) and
  behind `#[cfg(windows)]` — not refactoring. 35 cfg sites and several working branches
  (FPM-over-TCP, FastCGI-over-TCP, named-pipe listener, `interprocess`/`fs4` deps,
  `icon.ico`, `TitleBarStyle::Windows`) already exist.
- The three "fundamentally hard" areas are sequenced late enough that their
  dependencies exist first: **privilege/elevation** in Phase 4 (needs IPC + paths +
  helper plumbing from Phases 1–3), **daemon service lifecycle** in Phase 5 (needs a
  daemon that actually runs workloads, Phases 1–2), **self-update** in Phase 6 (needs
  the installer/bundling story it rides on).
- **Compile at every phase — never half-flip `active`.** `yerd-platform`'s
  `os/mod.rs:10,34` selects the OS impl via cfg, and the `active` re-export must
  ALWAYS name a type that fully implements the trait. A phase may only flip `active`
  from the `unsupported` stub to the Windows type in the SAME change that adds that
  type's trait impl — flipping the alias ahead of (or separately from) the impl
  breaks the Windows build mid-phase.
- **Green Windows CI must not be vacuous.** The load-bearing integration tests are
  currently `#[cfg(unix)]` and compile to nothing on Windows
  (`bin/yerdd/tests/lifecycle.rs:11`, `bin/yerd/tests/cli_e2e.rs`,
  `wp_shim_e2e.rs`, `cover_shim_e2e.rs`) — a green `windows-latest` leg proves
  nothing until they're ported. Each phase's DoD counts only tests that actually run
  on Windows; the ports are scheduled in Phases 1 and 5 below.
- Sibling repos (`yerd-php`, `yerd-services`, `yerd-php-ext`) already publish Windows
  artifacts — the consumer side is all that's missing. No cross-repo contract changes
  are expected; if one appears, call it out per CLAUDE.md.

---

## Phase 1: Foundations — compile, resolve, locate, talk

**Goal:** The whole workspace builds and its tests pass on Windows; the CLI can talk to
the daemon over a named pipe; artifact resolution knows Windows exists. Everything later
stands on this.

**Scope**

- **Artifact enums** — add `Os::Windows` + arch arms to `yerd-php/src/release.rs` and
  `yerd-services/src/release.rs` (`current_os_arch()` stops erroring); add
  `Platform::current()` Windows variant and `is_windows_*` matchers to
  `yerd-update/src/artifact.rs`. Filenames follow the already-published sibling schemes
  (`<svc>-<ver>-windows-<arch>.tar.gz`, `yerd-dump-<minor>-windows-<arch>.dll`).
- **Paths** — real Windows impl of the `Paths` trait in `yerd-platform`
  (`paths.rs::resolve`): `%APPDATA%\yerd` (config), `%LOCALAPPDATA%\yerd` (data/cache),
  `%TEMP%\yerd` (runtime). Fix the `for_user` non-Unix branch that fabricates
  Linux-style `~/.config` paths. Note: `for_user` is more than a path swap —
  `crates/yerd-platform/src/paths.rs:51` takes `uid: u32` and line 52 hardcodes
  `/tmp/yerd-{uid}` **unconditionally** (not `#[cfg]`-split like config/data), and
  `uid` is meaningless on Windows. Either cfg-gate the runtime line or rethink the
  signature (e.g. an OS-specific user identity); its only non-test caller is
  `bin/yerd/src/uninstall.rs:82`, so the blast radius is small but the Unix contract
  (elevation reconstructs paths from `SUDO_UID`) must be preserved.
- **IPC** — deterministic pipe name (replacing the PID-based `yerd-{pid}` name) in the
  daemon's `build_ipc_listener`; wire the currently `#[cfg(unix)]`-only client exchange
  in `yerd-ipc` so the Windows stub (`DaemonUnreachable "pipe name non-deterministic"`)
  goes away. **Cross-session note:** the user chose a Windows Service (Phase 5), which
  runs in session 0 while the GUI/CLI run in the interactive session, so the pipe must
  be reachable across sessions — use a **global** deterministic name (e.g. keyed on the
  installing/interactive user's SID) with an **explicit DACL** granting that user, NOT
  a session-local `Local\` name. Getting the DACL right is required here (not deferrable
  to Phase 4) because it's the only thing restricting who can drive the daemon; if
  `interprocess` can't set the security descriptor, drop to raw Win32
  (`CreateNamedPipe` + `SECURITY_ATTRIBUTES`) for the listener.
- **Lifecycle integration test on Windows** — port `bin/yerdd/tests/lifecycle.rs`
  (currently `#[cfg(unix)]` at line 11) so the daemon-up / IPC-ping / clean-shutdown
  round-trip is *tested* on Windows, not just manually asserted. This is what makes
  the new CI leg a real regression net rather than a vacuous green check.
- **Platform identity** — daemon `hostPlatform()` returns `windows`; add `windows` to
  the frontend gate in `apps/yerd-gui` `usePlatform.ts` (tiny, unblocks all later GUI
  testing).
- **Stub honesty** — Windows binds to a new `os/windows.rs` in `yerd-platform` that
  delegates to `unsupported.rs` for everything not yet implemented, so later phases
  fill in trait methods one at a time (respecting the "never half-flip `active`" rule
  above).
- **CI** — add a `windows-latest` leg to `.github/workflows/ci.yml` running
  `cargo check`/`cargo test` (workspace, no bundling yet). With the lifecycle port
  above, this is the regression net for every subsequent phase.

**Explicitly out of scope:** any process spawning/supervision changes, DNS, TLS trust,
elevation, extraction changes, bundling, PHP/CLI shims (Phase 5).

**Dependencies:** none.

**Key risks / decisions**

- Pipe naming/ACL detail: global SID-keyed name + explicit DACL is required now (the
  Phase 5 Windows Service runs cross-session); confirm `interprocess` exposes enough to
  set the security descriptor, else drop to raw Win32 for the listener in this phase.
- `for_user` fix touches Unix code paths too (elevation reconstructs paths from
  `SUDO_UID`) — must not change Unix behavior; cover with tests. Signature change vs
  cfg-gating the runtime line is an implementation call, but don't ship a Windows
  `for_user` that silently returns a `/tmp/yerd-{uid}` runtime dir.

**Definition of done:** `cargo test` green on ubuntu/macos/windows CI, **with the
ported `lifecycle.rs` actually executing on the Windows leg** (verify it's not
cfg'd out). On a Windows machine, `yerdd` starts, `yerd status` (or equivalent ping)
round-trips over the named pipe, config/data dirs appear under the correct
`%APPDATA%`/`%LOCALAPPDATA%` locations. macOS/Linux behavior byte-identical.

---

## Phase 2: Run things — supervision, extraction, PHP, services

**Goal:** The daemon can download, extract, run, and *cleanly kill* real workloads on
Windows: PHP-FPM per site, database/cache services, mail. This is where Job Objects —
one of the four hard risk centers — lands, before anything depends on process trees
being torn down correctly.

**Scope**

- **Job Objects** — `yerd-supervise/src/real.rs`: create a Job Object per supervised
  process (`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`), assign children, terminate via
  `TerminateJobObject` — replacing the current direct-child-only kill that leaks
  descendants/workers. Cfg-gate the Unix `process_group(0)` `CommandExt` calls in the
  `yerd-php`/`yerd-services`/`yerd-tunnel` managers.
- **Postgres shutdown** — no SIGINT equivalent on Windows: shell out to `pg_ctl stop -m fast`
  (binary ships in the yerd-services tarball), fall back to Job Object termination.
  MVP-acceptable: forced stop with a doctor warning if `pg_ctl` fails.
- **Extraction** — add a `.zip` path alongside tar.gz (the `zip` crate is already a
  workspace dep) where Windows artifacts require it; skip `chmod`/mode-bit application
  on Windows in the shared extraction code and the self-update `tar -xpf` path (full
  self-update is Phase 6, but extraction plumbing lands here).
- **PHP runtime** — exercise and harden the existing `#[cfg(windows)]` branch in
  `yerd-php` `listen.rs` (bind `127.0.0.1:0` → `TcpLoopback`): the documented racy
  bind/rebind window relies on manager retries — add a test and tighten retry/backoff.
  `yerd php install <ver>` consumes the published Windows static PHP artifacts;
  `yerd-dump` `.dll` loads into FPM. (The `php`/`composer`/`wp` shims that *dispatch*
  to these runtimes are Phase 5 — this phase only makes the daemon able to install
  and run them.)
- **Services** — `yerd service install/start/stop` for Valkey/Redis, MySQL, MariaDB,
  Postgres, Meilisearch, versitygw using the published `-windows-` tarballs, supervised
  under Job Objects. `yerd-mail` likewise.
- **FastCGI** — the existing `fcgi.rs` / `fastcgi_probe.rs` `#[cfg(windows)]`
  TCP branches get exercised end-to-end here (probe against a real FPM).

**Explicitly out of scope:** serving sites over 80/443 (Phase 3), `.test` DNS, TLS
trust, tunnel end-to-end (binary support only; UX in Phase 5), daemon
restart/autostart, PHP/CLI shims (Phase 5).

**Dependencies:** Phase 1 (artifact enums, paths, IPC).

**Key risks / decisions**

- **Job Objects is the high-risk item.** Nested-job semantics are fine on Win8+, but
  verify a killed MySQL/Postgres doesn't leave orphans; add an integration test that
  spawns a child-spawning process and asserts the tree dies.
- Windows Defender / file-locking flakiness on extraction and first-run of downloaded
  binaries — add retry-on-sharing-violation in extraction; document, don't over-solve.
- Which Windows artifacts are zip vs tar.gz — confirm against actual sibling releases
  before writing the dispatch (contract check, per CLAUDE.md).

**Definition of done:** On Windows: install PHP 8.x, install+start+stop a service
(include Postgres for the shutdown path), FPM comes up per site config and the FastCGI
probe passes, `yerdd` shutdown leaves zero orphaned processes (verified by a test
that runs on the Windows CI leg). CI stays green on all three OSes.

---

## Phase 3: Serve sites — ports, TLS trust, proxy end-to-end

**Goal:** A site created on Windows is reachable at `https://127.0.0.1` (or a
hosts-file-mapped name) with a locally trusted certificate. Everything except `.test`
auto-DNS, which waits for elevation (Phase 4).

**Scope**

- **PortBinder** — Windows impl in `yerd-platform`: **direct bind 80/443** (non-admin
  can bind <1024 on Windows by default), reusing `pure/port_plan.rs`. `PortRedirector`
  = no-op on Windows (no setcap/pf analogue needed). Doctor check for the port already
  being taken (IIS/W3SVC, Skype legacy, `urlacl` reservations) with a friendly message.
- **TrustStore** — Windows impl: install the Yerd CA into the **CurrentUser Root**
  store via `CertAddCertificateContextToStore` (or `certutil.exe` fallback) + presence
  probe; uninstall path. CurrentUser store shows a one-time confirmation dialog but
  needs no UAC — deliberately chosen so this lands before elevation exists.
- **Proxy** — no code change expected (`yerd-proxy` is pure tokio-rustls); validate
  HTTP→HTTPS, SNI per site, FastCGI-over-TCP upstreams from Phase 2 end-to-end on
  Windows.
- **Site creation** — exercise the existing `#[cfg(windows)]` branches in `create_site`
  and confirm site roots with Windows paths flow through config → proxy → FPM env
  (`SCRIPT_FILENAME` path separators are the classic bug — add a test).
- **GUI smoke** — with `usePlatform` returning `windows` (Phase 1), sites list,
  create-site, and PHP-version-per-site flows work in the Tauri dev app on Windows.

**Explicitly out of scope:** `.test` resolution via hosts/NRPT (Phase 4 — needs
elevation), machine-store CA install, ACL hardening of the CA key (Phase 4 TODO at
minimum, MVP-deferrable), **Windows Firefox/NSS certutil trust** — `nss_exec.rs` is
`#[cfg(unix)]` with a hardcoded `/usr/bin/certutil`, and a Windows NSS impl
(locating/shipping certutil, profile discovery) is non-trivial. Explicitly OUT of the
MVP: Firefox-on-Windows users get a documented manual-trust note; tracked as a
post-MVP TODO. Mac/Linux NSS behavior is unaffected.

**Dependencies:** Phase 1 (paths/IPC), Phase 2 (FPM + FastCGI running).

**Key risks / decisions**

- **Decision for user:** CurrentUser vs LocalMachine cert store. Recommendation:
  CurrentUser for MVP (no elevation, per-user matches rootless philosophy); offer
  machine store later via the Phase 4 helper if Edge/corp policy issues surface.
- Direct-bind assumption: verify no `http.sys` URL reservation conflicts on typical
  dev machines; the fallback (bind 8080/8443 + `netsh interface portproxy`) is the
  contingency, not the plan.

**Definition of done:** On Windows: `yerd site create`, browse
`https://<site>.test` with a hosts-file entry added *manually* (or `curl --resolve`)
— green padlock, PHP page renders, HTTPS cert chains to the user-store CA. Cert
install/uninstall/probe round-trips. CI green.

---

## Phase 4: Privilege & elevation — UAC helper, NRPT wildcard DNS, uninstall

**Goal:** Land hard-area #1 (privilege/elevation) now that IPC, paths, and the ops it
protects exist — and immediately spend it on the thing that needs it: `.test` DNS via
NRPT wildcard rules (user-chosen over the hosts file).

**Scope**

- **Elevation flow** — `bin/yerd/src/elevate.rs`: Windows path launches
  `yerd-helper.exe` per-operation via `ShellExecuteEx` with the `runas` verb (UAC
  prompt), preserving the audited one-shot-helper model rather than switching to an
  admin manifest (keeps parity with the Unix sudo/osascript design: granular,
  auditable ops). Marshal op arguments the same way as Unix; no `SUDO_UID` analogue
  needed since Windows paths are user-relative (Phase 1).
- **Privilege checks** — `bin/yerd-helper/main.rs`: replace the euid check with
  `GetTokenInformation(TokenElevation)`; `bin/yerd` side gains an
  `IsUserAnAdmin`/token-elevation probe for doctor + preflight messaging. Remove the
  exit-78 unsupported stubs.
- **Ownership/ACL checks** — the `MetadataExt::uid` ownership assertions in
  `elevate.rs` get a Windows equivalent via `GetNamedSecurityInfo` (owner-SID
  comparison). Keep it to the checks elevation actually performs — full
  `secure_fs.rs` ACL enforcement (0o600 analogues for CA key, pipe DACL hardening)
  is written here **if cheap**, else logged as an explicit tracked TODO (MVP-accepted
  gap per report).
- **DNS resolver (NRPT wildcard — user choice)** — new Windows `ResolverInstaller`
  in `yerd-platform` + a helper op that installs/removes a single **NRPT rule**
  (`Set-DnsClientNrptRule -Namespace ".test" -NameServers 127.0.0.1`, removed via
  `Remove-DnsClientNrptRule`, or the equivalent `HKLM\...\DnsPolicyConfig` registry
  writes) so the whole `*.test` namespace resolves to the bundled hickory server —
  no per-site edits, true wildcard. NRPT config lives under HKLM, so the write is
  **elevation-gated** and runs through the Phase 4 helper. The bundled hickory server
  (already running) is the resolver the rule points at, so it must answer `.test`
  queries for arbitrary sites (confirm it does, not just registered ones). Idempotent
  install/repair (rule present + correct nameserver). Because it's namespace-wide, a
  single rule is installed once (at setup / first site) rather than per-site — far
  fewer UAC prompts than the hosts approach would have needed.
- **Uninstall** — `bin/yerd/src/uninstall.rs` Windows path: remove the NRPT rule, CA
  from cert store, data dirs (service/autostart cleanup joins in Phase 5). Uses the
  Phase 1 `for_user` fix (its only non-test caller, `uninstall.rs:82`).

**Explicitly out of scope:** hosts-file DNS fallback (not needed given NRPT); daemon
service registration (Phase 5); any LocalMachine cert store work.

**Dependencies:** Phase 1 (IPC/paths for helper plumbing), Phase 3 (trust store +
serving stack that DNS makes reachable).

**Key risks / decisions**

- **NRPT (user-chosen):** confirm the bundled hickory server answers `.test` queries
  for arbitrary hostnames (NRPT sends the whole namespace to it, not just registered
  sites) — if it only knows registered sites, unregistered `*.test` lookups NXDOMAIN.
  Verify NRPT rules apply without a reboot/`ipconfig /flushdns` (they should take
  effect immediately). One namespace rule replaces per-site edits, so DNS setup is a
  single elevated op, not one-per-site.
- UAC is coarse (full admin token) vs Unix's granular sudo — the audited one-shot
  helper pattern is the containment; keep helper ops minimal and validated.
- ShellExecuteEx gives no stdout/stderr from the elevated child — helper must report
  results via exit code + a result file in the runtime dir (design this small).

**Definition of done:** On Windows: after the one-time elevated DNS setup, any
`https://<anything>.test` resolves and loads with no manual edit and no further UAC
per site. Helper refuses to run un-elevated ops without the token check passing.
`yerd uninstall` removes the NRPT rule and CA, leaving nothing behind. Unix elevation
untouched, CI green.

---

## Phase 5: Daemon lifecycle, autostart, CLI/PATH & PHP shims

**Goal:** Land hard-area #2: the daemon behaves like a first-class background citizen —
restarts, shuts down cleanly, starts at login — and the *entire* command-line surface
is installable/discoverable the Windows way: not just `yerd.exe`, but the `php`,
`php<ver>`, `composer`, `wp`, `laravel`, and coverage shims that are a core product
feature and today exist only for Unix.

**Scope**

- **Restart** — `bin/yerdd/src/main.rs`: replace the rejected-on-non-Unix POSIX
  `exec()` in-place restart with spawn-new-then-exit on Windows (new instance waits on
  the old pipe/process handle before binding — reuse the Phase 1 deterministic pipe
  name as the readiness signal).
- **Shutdown signals** — `signals.rs`: `SetConsoleCtrlHandler` for
  CTRL_C/CTRL_BREAK/CTRL_CLOSE alongside the existing Ctrl-C; graceful teardown drains
  Phase 2 Job Objects.
- **PHP/CLI shims (CRITICAL — currently in no phase, entirely `#[cfg(unix)]`)** —
  the `php`, `php<ver>`, `composer`, `wp`, `laravel`, and coverage commands are
  delivered by shim dispatch in `bin/yerd/src/main.rs:11-30` via modules gated
  `#[cfg(unix)]` in `bin/yerd/src/lib.rs:13-31` (`cli_shim.rs`, `wp_shim.rs`,
  `composer_shim.rs`, `laravel_shim.rs`, `cover_shim.rs`), all exec-based through
  `std::os::unix::process::CommandExt`. Shim *installation* is likewise Unix-only:
  in `bin/yerdd/src/php_install.rs:582-585`, `set_default_shim` is a non-unix no-op
  and `place_symlink`/`reconcile_shims`/`versioned_shim_name` are `#[cfg(unix)]`
  (symlink-based). Without this work a Windows user gets NO `php`/`composer`/`wp`
  on PATH and no per-site PHP version dispatch — a core feature silently missing.
  Windows delivery needs two changes:
  1. **Dispatch:** replace `exec()` with spawn-and-wait (`std::process::Command` +
     propagate exit status/Ctrl-C) in each shim module, un-gating them for Windows.
  2. **Delivery mechanism (`.cmd`/`.bat` wrappers — user choice):** generate small
     `.cmd` wrapper files in the shim dir on PATH (e.g. `php.cmd`, `php83.cmd`,
     `composer.cmd`, …) that invoke `yerd.exe` in shim mode forwarding all args
     (`@echo off` + `"%~dp0yerd.exe" <shim> %*` style). No symlinks (Developer-Mode
     requirement), no exe-copies (AV-rescan churn on version bumps). Wrappers are
     cheap to (re)write and stable across PHP-version changes since the version
     resolution happens inside `yerd.exe` at runtime.
  `php_install.rs` gains the Windows counterparts of
  `place_symlink`/`reconcile_shims`/`versioned_shim_name`/`set_default_shim` that
  write/reconcile `.cmd` wrappers instead of symlinks.
- **Shim e2e tests on Windows** — port `bin/yerd/tests/cli_e2e.rs`,
  `wp_shim_e2e.rs`, and `cover_shim_e2e.rs` (all currently `#[cfg(unix)]`, compiling
  to nothing on Windows) alongside the shim work so the Windows CI leg actually
  exercises dispatch, per-site version selection, and exit-code propagation.
- **Autostart / service control (Windows Service — user choice)** —
  `yerd-service-ctl` Windows backend registers `yerdd` as a real **Windows Service**
  (SCM) so it starts at boot and survives logoff, matching the systemd-system /
  launchd-daemon model rather than a per-user agent. Register/unregister via the SCM
  APIs (`CreateService`/`DeleteService`/`ChangeServiceConfig`) or `sc.exe`, run
  through the Phase 4 elevated helper since service install needs admin. `yerdd` gains
  a service-mode entry point that talks to the SCM dispatcher
  (`StartServiceCtrlDispatcher` / reports `SERVICE_RUNNING` / handles
  `SERVICE_CONTROL_STOP`) — the existing console `main` path stays for `cargo run`
  and foreground use. Mind **Session 0 isolation**: the service runs headless in
  session 0 and must not assume a desktop; the GUI stays a normal per-user app (Tauri
  autostart Run key) that connects to the service over the Phase 1 named pipe — so the
  pipe name/ACL must be reachable cross-session (a fixed name + explicit DACL granting
  the interactive user, not a session-local `Local\` name — revisit the Phase 1 pipe
  naming for this). Process/status detection via the SCM (`QueryServiceStatusEx`)
  instead of `pgrep`.
- **CLI install / PATH** — no symlinks (Developer-Mode requirement): copy
  `yerd.exe` into `%LOCALAPPDATA%\Programs\yerd\bin` and edit the **user** PATH via
  the `HKCU\Environment` registry key + `WM_SETTINGCHANGE` broadcast, replacing the
  `~/.bashrc`/`.zshrc`/fish logic in `path_cmd.rs` on Windows. The shim dir (above)
  joins the same PATH entry so `php`/`composer`/`wp` resolve from a fresh shell.
- **TerminalLauncher** — Windows impl: Windows Terminal (`wt.exe`) → PowerShell →
  cmd fallback chain.
- **Doctor** — Windows-aware checks/messaging: elevation state, `yerdd` service
  registered + running (SCM query), port 80/443 reservation conflicts, NRPT rule
  present + pointing at the bundled resolver, cert presence, shim dir on PATH and
  `.cmd` wrappers consistent with installed PHP versions.
- **Tunnel** — exercise `yerd-tunnel`'s existing `#[cfg(windows)]` cloudflared branch
  end-to-end (sibling publishes the Windows binary; supervision comes free from
  Phase 2).

**Explicitly out of scope:** installer-time service registration (an NSIS hook in
Phase 6 may *invoke* the Phase 5 register op), self-update, GUI copy audit.
**Deferred as TODO (not scheduled):** `SystemMetrics` via `GetProcessMemoryInfo` —
currently returns `None` on Windows, purely cosmetic; tracked TODO, not MVP work.

**Dependencies:** Phase 1 (cross-session pipe name/DACL for restart handshake and
service↔GUI IPC), Phase 2 (Job Objects for clean teardown, installed PHP runtimes for
shims to dispatch to), Phase 4 (elevated helper — service register/unregister needs
admin; doctor's elevation checks).

**Key risks / decisions**

- **Windows Service (user-chosen) — Session 0 is the risk.** The service has no
  desktop; anything that assumed a user session (env vars like `%LOCALAPPDATA%`,
  `%USERPROFILE%`) must be resolved for the correct target user, not the service
  account. Decide the service account (recommend `LocalService`/a dedicated account
  vs the interactive user) and how it locates the user's data dirs — this is the
  thorniest part and may warrant its own mini-writeup during Phase 5 planning. The
  cross-session pipe DACL (Phase 1) is what lets the per-user GUI reach the session-0
  daemon.
- Service register/unregister needs elevation → routed through the Phase 4 helper;
  a fresh dev run (`cargo run -p yerdd`) must still work as a plain foreground console
  process without SCM.
- Spawn-and-wait `.cmd`-wrapper shims (no `exec()`) must propagate exit codes and
  Ctrl-C to the child correctly — easy to get subtly wrong; the ported e2e tests are
  the guard. The `cmd.exe` hop must not swallow the child's exit code (`%*`
  forwarding + `exit /b`).
- Spawn-new+exit restart has a handoff window (pipe briefly gone) — CLI already
  retries `DaemonUnreachable`? Verify; add bounded retry if not.
- PATH registry edits are notoriously easy to corrupt (REG_EXPAND_SZ vs REG_SZ,
  truncation) — read-modify-write carefully, test with pre-existing long PATHs.

**Definition of done:** On Windows: `yerd` AND `php`/`composer`/`wp`/`laravel` on
PATH from a fresh shell (via `.cmd` wrappers); `php -v` in a site dir dispatches to
that site's PHP version; `yerdd` is registered as a Windows Service that starts at
boot and the per-user GUI reaches it over the cross-session pipe; `yerd restart`
works; service stop / Ctrl-C shuts everything down with no orphans; `yerd doctor`
reports a healthy Windows-specific checklist. The ported shim e2e tests run (not
cfg'd out) on the Windows CI leg. CI green on all three OSes.

---

## Phase 6: Packaging, self-update, GUI polish — shippable Windows build

**Goal:** Land hard-area #3 (self-update of a running exe) inside the packaging story
it depends on, and finish the user-facing surface: an installable, updatable,
Windows-idiomatic Yerd.

**Scope**

- **Tauri bundling** — `apps/yerd-gui/tauri.conf.json` (or a `bundle-windows` variant
  per the existing per-OS conf pattern): add `nsis` to `bundle.targets`
  (`icon.ico` already exists); WebView2 bootstrapper set to `downloadBootstrapper`;
  stage `yerd.exe`/`yerdd.exe`/`yerd-helper.exe` as
  `externalBin` with the `-<triple>.exe` naming the existing build legs use.
- **CI release leg** — `.github/workflows/build.yml`: add the Windows leg mirroring
  the bespoke macOS/Linux legs (build 3 bins + Tauri NSIS bundle, sign later/TODO).
  NSIS install hooks call the Phase 5 `yerd-service-ctl` register op and PATH install
  (installer runs elevated anyway, so first-run UAC prompts shrink); uninstaller hook
  calls `yerd uninstall` (Phase 4).
- **Self-update** — `yerd-update`: replace the Unix `tar -xpf`+chmod flow on Windows
  with **staged-rename-on-restart** for the CLI/daemon (`.exe.new` beside the binary;
  the Phase 5 spawn-new+exit restart applies it — old exe renames away while running,
  which Windows allows), and **download-installer-launch-and-exit** for the GUI
  (NSIS silent mode `/S`). Uses Phase 1 `is_windows_*` matchers and Phase 2 zip
  extraction. The `.cmd` wrapper shims (Phase 5) need no rewrite on self-update since
  they only invoke `yerd.exe` by relative path — version resolution stays inside the
  updated binary.
- **Frontend polish** — audit `GeneralView.vue`/`WelcomeView.vue` and friends for
  Unix-isms (sudo, Terminal.app, `~/.bashrc`, `/usr/local`) with Windows copy;
  Windows PATH-install UI wired to the Phase 5 op; keyboard shortcuts already route
  Ctrl on non-Mac; verify Tauri single-instance + autostart plugins on Windows.
- **Docs** — README/docs note Windows support status and known MVP limitations
  (per-user cert store, ACL hardening TODO, no Firefox/NSS auto-trust, no system
  metrics).

**Explicitly out of scope (post-MVP backlog):** MSI/WiX target, code signing +
SmartScreen reputation, LocalMachine cert store, real ACL enforcement in
`secure_fs.rs`, winget/scoop manifests, Windows Firefox/NSS certutil trust
(Phase 3 TODO), `SystemMetrics` via `GetProcessMemoryInfo` (Phase 5 TODO).

**Dependencies:** Phases 1–5 (everything: bundling stages all three bins; self-update
needs restart semantics from Phase 5, extraction from Phase 2, matchers from
Phase 1; installer hooks need Phase 4/5 ops; shim refresh needs Phase 5's delivery
mechanism).

**Key risks / decisions**

- **Self-update is the risk center.** Staged-rename is simple but has failure modes
  (locked file by AV, crash between rename and spawn) — keep the `.old` around one
  cycle for rollback. GUI-installer handoff must not fight the daemon's own update.
- **Decision for user:** NSIS only for MVP (recommended — Tauri's default, simpler)
  vs also MSI (enterprise). Unsigned binaries will trip SmartScreen — decide whether
  signing is a launch blocker or accepted for early access.
- NSIS elevation model (per-machine vs per-user install) interacts with the
  per-user design everywhere else — recommend **per-user NSIS install**
  (`installMode: currentUser`) to stay consistent and mostly UAC-free.

**Definition of done:** CI produces a Windows NSIS installer; on a clean Windows VM:
install → GUI launches → daemon autostarts → create site → browse `https://x.test` →
`php -v` dispatches correctly → trigger a version update → everything (shims
included) comes back on the new version. Uninstall leaves a clean machine. All
prior-phase functionality still passing; ubuntu/macos CI green.

---

## Summary

| Phase | Title | Effort | Headline risk |
|---|---|---|---|
| 1 | Foundations: enums, paths, IPC, lifecycle test, CI | M | Named-pipe naming/ACL details in `interprocess` |
| 2 | Run things: Job Objects, extraction, PHP, services | L | Process-tree teardown correctness (Job Objects) |
| 3 | Serve sites: direct-bind ports, TLS trust, proxy e2e | M | Cert-store choice / port 80 reservation conflicts |
| 4 | Privilege & elevation: UAC helper, NRPT wildcard DNS, uninstall | L | Coarse UAC vs granular helper; hickory answering `*.test` |
| 5 | Daemon lifecycle, Windows Service, PATH, `.cmd` PHP shims | L | Windows Service Session-0 / cross-session pipe |
| 6 | Packaging & self-update: NSIS, staged rename, GUI polish | L | Updating a running exe; unsigned-binary SmartScreen |

## Decisions — resolved and open

Resolved by the user (2026-08-02):

1. **Cert store (Phase 3):** ✅ **CurrentUser Root** store for the Yerd CA.
2. **`.test` DNS (Phase 4):** ✅ **NRPT wildcard** (`Set-DnsClientNrptRule`, true
   `*.test`), elevation-gated. Hosts-file approach dropped.
3. **Daemon autostart (Phase 5):** ✅ **Windows Service** (SCM), starts at boot,
   register needs admin. Task Scheduler dropped. Drives the Phase 1 cross-session
   pipe requirement.
4. **Shim delivery (Phase 5):** ✅ **`.cmd`/`.bat` wrappers** invoking `yerd.exe` in
   shim mode. Exe-copies and symlinks dropped.

Still open (decide at Phase 6 planning):

5. **Packaging & signing (Phase 6):** NSIS-only vs NSIS+MSI, and whether unsigned
   binaries (SmartScreen warnings) are acceptable for early access or code signing is
   a launch blocker. Leaning: NSIS-only, per-user install; decide signing before
   public launch.
