# Windows port - out-of-scope follow-ups (NOT committed)

Working scratch file created during Phase 1. Do not stage/commit.

## Deferred improvements discovered during Phase 1

- **`crates/yerd-php/tests/supervisor_states.rs` is now `#![cfg(unix)]`.**
  It uses the real `ActivePortBinder`, which is the `Unsupported` stub on
  Windows, so every test hit `PhpError::Bind { Unsupported }`. A better fix
  (more Windows coverage) is to inject a *fake* `PortBinder` returning a bound
  loopback port so the tests exercise the Windows `Listen::TcpLoopback` planner
  path too. Deferred to avoid gold-plating Phase 1; revisit when a real
  `WindowsPortBinder` lands.

- **`bin/yerd-helper` internals are dead code on Windows** (main returns
  exit-78 before dispatch). Silenced with a crate-level
  `#![cfg_attr(not(any(linux, macos)), allow(dead_code, unused_imports))]`.
  When the Phase 4 Windows privilege model lands, the ops should be wired up and
  the blanket allow removed.

- **CGI `SCRIPT_FILENAME`/`DOCUMENT_ROOT` separators on Windows**
  (`crates/yerd-proxy/src/pure/cgi_params.rs`). `build_params` derives these via
  `document_root.join(...).to_string_lossy()`, which emits `\` on Windows;
  `SCRIPT_NAME` already normalizes with `.replace('\\', "/")` but the filename /
  doc-root do not. Four byte-exact tests were scoped `#[cfg(unix)]`. Phase 2
  (Windows PHP request serving) must decide the correct Windows CGI path form
  (php-cgi likely accepts native separators) and add matching Windows tests.

- **Windows DB/search config path separators** (`crates/yerd-services`).
  `config_render.rs` (my.cnf/postgresql.conf) and `service.rs` (meilisearch
  launch args) build paths via `datadir.join(...).display()`, emitting `\` on
  Windows. Five goldens were scoped `#[cfg(unix)]`. Phase 5 (Windows service
  binaries) must decide the correct separator form for each config/CLI and add
  Windows tests. Also `yerd-services/tests/supervisor_states.rs` is now
  `#![cfg(unix)]` (real `ActivePortBinder` preflight), same as yerd-php.

- **yerdd PHP install/discovery tests are `#[cfg(unix)]` on Windows.**
  ~14 `ipc_server` tests (list/set-default/uninstall/available/poll_and_refresh)
  plus a few others use `fake_install`, which writes the Unix bundle layout
  (`bin/php`, `sbin/php-fpm`). `yerd_php::discover_bundled` on Windows looks for
  `php-fpm.exe` (a different layout). Phase 2 (Windows PHP packaging: repackaged
  windows.php.net `php.exe`/`php-cgi.exe`) should implement the Windows install
  layout and add matching Windows install/discovery tests, then drop these
  `#[cfg(unix)]` gates. Also gated for the same/related reasons:
  `services::pick_free_port_skips_reserved` (Unsupported binder),
  `tunnel::install::find_in_paths_ignores_non_executable_file` (Unix exec bit),
  `mutate::unpark_drops_proxy_rules_under_root` (native `MAIN_SEPARATOR` with a
  Unix-path fixture).

- **Frontend build/test blocked by missing dep in this environment.**
  `apps/yerd-gui/src/lib/mailLinks.ts` imports `isomorphic-dompurify` /
  `dompurify`, both declared in `package.json` but absent from `node_modules`
  here, so `npm run build` (vue-tsc) and the `mailLinks.test.ts` transform fail.
  Unrelated to the Windows port. Run a full `npm install` (or `npm ci`) in
  `apps/yerd-gui` to restore them; the Item 7 `usePlatform` change and its test
  pass cleanly (vue-tsc flagged only the mailLinks missing-module errors).

- **`clippy::result_large_err` fires only on Windows** for several
  `Result<_, yerd_ipc::Response>` functions in `bin/yerdd` (`ipc_server.rs`,
  `services.rs`). Suppressed with `#[cfg_attr(windows, allow(...))]` to keep the
  Unix diff empty. The real fix (boxing large `Response` variants) is a
  cross-cutting `yerd-ipc` change out of Phase 1 scope.

## Deferred / found during Phase 2 (NOT committed)

- **php-cgi post-plan port-race replan-once not implemented (Item 5b hardening).**
  `crates/yerd-php/src/manager.rs`: the plan asks that, on Windows, a first
  `ensure` drive failing with `HealthCheckTimedOut`/`PermanentFailure` (because
  the baked ephemeral port was re-taken between `drop(bound)` and php-cgi's own
  bind) replan a fresh port once and retry the whole `ensure`. The plan-time
  jitter backoff (in `plan_listen`) is implemented; the post-plan single replan
  is deferred as edge-case hardening. The pool would currently exhaust its 3
  restart attempts on the same baked port in that rare race. Follow-up: extract
  the ensure body into `ensure_once` and wrap it with one Windows-only replan.

- **MVP: one `php-cgi.exe` per PHP version = one concurrent PHP request per
  version** (php-cgi cannot pre-fork on Windows; `PHP_FCGI_CHILDREN` is
  unsupported). Herd runs a small worker pool. Post-MVP: spawn N php-cgi workers
  on N ports per version and round-robin in the `yerd-proxy` `PhpFpmTcp` backend.
  Not Phase-2 scope (WINDOWS_PLAN deviation #6).

- **`bin/yerdd/src/wordpress_versions.rs:230` test panics on a low-uptime host.**
  `available_versions_falls_back_to_stale_cache_on_fetch_error` builds a "stale"
  timestamp with `Instant::now().checked_sub(CACHE_TTL + 1s).unwrap()`. On
  Windows shortly after boot the monotonic clock is smaller than `CACHE_TTL`, so
  `checked_sub` returns `None` and the `unwrap()` panics. Pre-existing, unrelated
  to Phase 2 (reproduces on the untouched baseline). Fix: guard the test with a
  saturating/`unwrap_or(Instant::now())` fallback, or skip when the clock is too
  young. Left untouched to stay in scope. (Re-observed in Phase 3: this machine's
  uptime was ~4.2h < the 12h CACHE_TTL, so it is the one workspace-test failure;
  every other test is green. Skip it with
  `--skip available_versions_falls_back_to_stale_cache_on_fetch_error`.)

## Deferred / found during Phase 3 (NOT committed)

- **Windows Firefox/NSS trust is a Phase 6 non-goal (locked).**
  `WindowsTrustStore::install_firefox_nss` / `uninstall_firefox_nss` return
  `Unsupported`, and `browser_ca_trust` stays the trait default. Edge/Chrome/
  Chromium follow the CurrentUser Root store the Windows `TrustStore` manages, so
  they are covered; Firefox keeps its own NSS profile store (unless
  `security.enterprise_roots.enabled`). Phase 6 adds a Windows NSS path, per the
  master plan.

- **`storage:link` symlink serving on Windows is unvalidated (post-MVP).**
  The four `#[cfg(unix)]` symlink-fixture proxy integration tests
  (`crates/yerd-proxy/tests/integration_http.rs` around lines 1137/1200/1266/1330,
  `std::os::unix::fs::symlink`) stay Unix-only: creating symlinks on Windows CI
  needs Developer Mode. Laravel `storage:link` behaviour served through the proxy
  on Windows is a recorded post-MVP validation TODO.

- **GUI in-app trust arm (Phase 3 Item 7, optional) deferred to a follow-up.**
  The CLI `yerd elevate trust` Windows arm is the Phase 3 DoD surface and is
  done. The macOS parity path (`apps/yerd-gui/src-tauri/src/mac_trust.rs`,
  `commands.rs` `trust_ca`/`untrust_ca`) was NOT mirrored for Windows because it
  is a cross-cutting change (a `#[cfg(windows)]` `commands.rs` arm calling
  `yerd_platform::ActiveTrustStore` in `spawn_blocking`, plus loosening the
  `platform === "macos"` gates in `EnvironmentCard.vue:200,257` and `client.ts`
  docs) and the frontend build/test is currently blocked in this environment
  (missing `isomorphic-dompurify`/`dompurify` node modules, see the Phase 1 note
  above), so the Vue side cannot be validated here. Implement + validate together
  in a follow-up so the in-app "Trust" button and its backend land as one change
  (never half-flip). `untrust_ca`'s "system-wide trust remains" return is always
  `false` on Windows (no second store we manage).

- **GUI manual smoke (Phase 3 Item 7, required-manual) not run headlessly.**
  Running the Tauri dev app against the dev daemon to verify sites list /
  create-site (needs Item 5) / PHP-version-per-site is a manual DoD gate that
  cannot be performed in this non-interactive session. The `EnvironmentCard`
  "use `yerd elevate` in a terminal" line is now accurate on Windows (post-Item
  3, minus `sudo`), and doctor chips render the Item 1 Windows remedies over IPC.

- **Doctor `PortRedirectStale`/`LanRedirectStale` remedy copy stays macOS-worded.**
  Item 1 made `CaNotTrusted`, `ForeignWebListener`, and `PortFallback` remedies
  Windows-aware. `redirect_stale_finding`/`lan_redirect_stale_finding`
  (`crates/yerd-doctor/src/lib.rs`) still emit `sudo yerd elevate ports/lan`, but
  they are driven by `port_redirect_targets`, which `WindowsPortRedirector`
  always reports as `None`, so these findings never fire on Windows in practice.
  The synthetic unit test at ~`:755` therefore passes unchanged on all OSes and
  was left as-is (no Unix assertion weakened). If a Windows pf-style redirect
  concept ever appears, reword these two remedies then.

## Deferred / found during Phase 4 (NOT committed)

- **Windows ACL hardening (the `secure_fs.rs` analogue) is unimplemented.**
  Phase 4 ships no `0o600`-equivalent DACLs for the CA key or the runtime dir.
  The elevated NRPT op takes only `--tld`/`--addr` (validated typed values, no
  file path), so there is nothing for an owner-SID / `GetNamedSecurityInfo` check
  to guard - the Unix `require_user_owned` gate was deliberately NOT ported
  (see PHASE4_PLAN §4.3). The one residual write, the advisory result file, is a
  fixed-name, fixed-content, `create_new` text file in `%TEMP%\yerd`; the accepted
  residual risk is a same-user process junctioning `%TEMP%\yerd` (no meaningful
  primitive). Revisit the owner-SID check the moment any Windows helper op takes a
  path argument, and add proper runtime-dir/CA-key DACL hardening.

- **Manual DoD gate: the elevated UAC + NRPT round-trip could not run in this
  non-interactive session.** The Step-0 spike ran at Medium integrity, so
  `Add-DnsClientNrptRule` (elevated) was never executed here. Verified indirectly
  from a pre-existing leftover `.test` rule (exact registry shape, braced-GUID
  `-Name`, unelevated `Get-DnsClientNrptRule`, unprivileged `127.0.0.1:53` bind).
  Still needs on-machine confirmation, elevated: (a) `yerd elevate resolver` -> one
  UAC prompt -> `Get-DnsClientNrptRule` shows one `.test` -> `127.0.0.1` rule ->
  `Resolve-DnsName whatever.test` hits 127.0.0.1 with no reboot; (b) that
  `Add-DnsClientNrptRule -Namespace '.test' -NameServers '127.0.0.1' -Comment
  'yerd'` writes exactly the value names the probe matches (`Name`,
  `GenericDNSServers`); (c) `Add-DnsClientNrptRule` autoloads under the helper's
  scrubbed env (`SystemRoot`+`windir` retained; `PSModulePath` pinned to the
  system modules dir, not inherited, so a user-writable module cannot hijack the
  cmdlet under elevation, M3); (d) `yerd
  unelevate resolver` / `yerd uninstall` leave no rule + no CA. The three spots
  that depend on (a)/(b)/(c) are called out in `pure/nrpt.rs`'s module doc.

- **GUI Windows elevate (Phase 6 frontend polish).** On Windows `yerd elevate` is
  itself unelevated (the helper raises UAC per-op), so the GUI can spawn the
  sibling CLI normally - no pkexec/osascript analogue. Deferred per the master
  plan: spawn `yerd elevate resolver` unelevated and un-gate the frontend "Fix"
  button for `windows`.

- **Doctor Phase 5 Windows checks.** Richer NRPT-rule detail (which server the
  `.test` rule points at) and naming a foreign `127.0.0.1:53` squatter are Phase 5
  per the master plan; Phase 4 only flips `resolver_installed` + the no-`sudo`
  remedy.

- **`runas 1.2.0` upstream arg-quoting bug.** Args containing space/tab/quote get
  their backslashes doubled by `CommandLineToArgvW`. Phase 4 dodges it by keeping
  every helper argv element in a closed charset (op tag, `--tld`, loopback addr,
  hex `--result-token`), guarded by `windows_helper_argv_is_runas_quoting_safe` in
  `bin/yerd/src/elevate.rs`. If a Windows helper op ever needs a path argument,
  fix upstream or replace `runas` with a first-party `ShellExecuteExW` wrapper
  (which would need an `unsafe` block, i.e. a `forbid` lift in `bin/yerd`).

## Deferred / found during Phase 5 Track A (NOT committed)

- **Track B (Windows Service / autostart) is intentionally unbuilt.** Everything
  in PHASE5_PLAN "Track B" is still pending the D1 service-account decision: the
  SCM dispatcher / service-mode `yerdd`, `yerd-service-ctl` Windows backend,
  `RegisterDaemonService`/`UnregisterDaemonService` helper ops + `yerd elevate
  service`, the service-registered doctor probe (`DaemonServiceNotRegistered`),
  and the uninstall service-unregister wiring. Also NOT added (Track B / not in
  the Track-A doctor subset): the `pure/win_svc.rs` `sc.exe sdset` SDDL helper,
  the `signals::SERVICE_STOP` `Notify` (the console signal path landed without
  it; Track B's SCM STOP handler will add it and trip the same watch), and the
  `DaemonElevated` doctor variant.

- **`ctrlc` was NOT adopted (D2).** The Windows shim wait path uses plain
  spawn-and-wait; a console Ctrl-C terminates the wrapper and child together
  (npm.cmd-style), which is acceptable and needs no new dep. Revisit only if
  exit-code correctness under interactive Ctrl-C is later required.

- **Node/Bun tool shims on Windows are skipped** (their Windows install pipeline
  isn't wired). `tools::reconcile_tool_shims`'s Windows arm writes `.cmd`
  wrappers only for the yerd-multicall tools (`composer`/`laravel`/`wp`) and
  `continue`s past Node/Bun. Post-MVP.

- **`yerd.exe` self-copy on `yerd path install` is best-effort, not a staged
  swap.** `path_cmd`'s Windows `copy_self_into_programs` tolerates a locked
  destination by staging `yerd.exe.new` beside it with a note; the full staged
  rename-on-restart is Phase 6. The full `yerd uninstall` cannot delete the
  running binaries (self-delete), noted in its residue summary.

- **`setx YERD_BIN <shim-dir>` is the `WM_SETTINGCHANGE` broadcast side-channel
  (D3).** `PATH` itself is written via `winreg` (setx would truncate long PATHs);
  `setx` only sets the incidental `YERD_BIN` marker for its documented broadcast
  side effect. If the marker variable is ever unwanted, the fallback is to drop
  the broadcast and print "open a new terminal after logging off/on".

- **Doctor Track-A subset only.** The Windows doctor gains the shim-dir-on-PATH
  check (Windows `path_needs_setup` arm → existing `BinDirNotOnPath`); the
  port-80/443, NRPT, and cert findings already fire from the Phase 3-4 probes. No
  new `DiagnosisCode` was needed. The service-registered / daemon-elevated checks
  are Track B.

- **cloudflared Windows arm is wired but its e2e is a manual DoD item.**
  `host_asset` now returns `cloudflared-windows-amd64.exe` (bare exe) for x86_64
  and `None` for arm64 (Cloudflare publishes none); `binary_path` uses
  `cloudflared.exe`. A real login/tunnel round-trip still needs a Cloudflare
  account and is unrun here.

## Phase 5B (Windows daemon autostart, HKCU Run key) - handoffs

- **Phase 6 GUI wiring for daemon-at-login.** The Windows arms of the GUI's
  `daemon_set_login` / `plan_start` / `daemon_stop` / `service_registered` /
  `is_set_up` (`apps/yerd-gui/src-tauri/src/autostart.rs` + `daemon` module) are
  still the `not(linux|macos)` "not supported" stubs. They should call the 5B
  backend: `yerd_service_ctl::{enable_at_login, disable_at_login,
  autostart_enabled}` for the login toggle and `ServiceCtl::{start, stop}` for
  the start/stop buttons. The GUI owns the "Start daemon at login" toggle; the
  backend is ready for it.

- **NSIS/Phase 6 uninstaller removes BOTH Run values.** `yerd uninstall` now
  removes the daemon's `Yerd Daemon` HKCU Run value (via
  `yerd_service_ctl::disable_at_login`), but the GUI's own `Yerd` Run entry
  (written by `tauri-plugin-autostart`) is still the installer's to remove,
  exactly as Unix uninstall leaves the GUI login item alone.

- **Phase 6 Windows self-update applier reuses `ServiceCtl`.** `bin/yerd`'s
  `apply.rs` is `cfg(macos|linux)` today; the Windows applier should drive
  `ServiceCtl::restart` (stop -> tasklist poll -> start) after the staged-rename
  swap, same as the Unix appliers.

- **Logon console flash (cosmetic wart).** The `serve --detach` relauncher trades
  a persistent console for a brief (~100-300 ms) console flash at logon (the
  console-subsystem `yerdd` is spawned, then respawns itself hidden and exits).
  Documented, not solved. Escape hatch if users complain: change the Run data to
  `%SystemRoot%\System32\conhost.exe --headless "<yerdd>" serve` and drop the
  relauncher (undocumented flag, hence not the default).

- **Optional future: an additive IPC `Shutdown` request.** Windows `stop` is a
  forced `taskkill /F` (Job Objects reap children), faithfully mirroring Unix's
  signal-based stop since there is no `Shutdown` wire request today. A graceful
  CLI-driven stop could add one additively; not needed now.

- **`autostart_enabled()` cross-user tasklist note.** `ServiceCtl::restart`'s
  `wait_for_exit` polls `tasklist /FI "IMAGENAME eq yerdd.exe"`, which is not
  user-scoped; a `yerdd.exe` under another user would only lengthen the wait loop
  (accepted). If multi-user hosts ever matter, add a `/FI "USERNAME eq ..."`.

## Phase 6 - manual Definition-of-Done gates (clean Windows VM; CI cannot cover)

These require a real clean Windows 10/11 VM and are NOT verifiable in CI or on the
dev host. Run them before promoting the Windows build past early access:

- [ ] Download `Yerd_Windows_x86_64_v<ver>.exe` -> SmartScreen "More info -> Run
      anyway" -> install completes with NO UAC prompt (per-user
      `%LOCALAPPDATA%\Yerd`).
- [ ] GUI launches; first run enables daemon autostart (HKCU Run `Yerd Daemon`)
      and, if toggled, GUI autostart (HKCU Run `Yerd`).
- [ ] Launch the app twice -> exactly one window (single-instance), one daemon.
- [ ] `yerd` and `php` resolve in a fresh terminal after `path install`.
- [ ] Create a site -> `https://x.test` shows a green padlock (Edge/Chrome);
      Firefox needs manual CA trust (documented).
- [ ] `php -v` dispatches the per-site version via the `.cmd` shim.
- [ ] Seed an older version, then run BOTH `yerd update --yes` AND the GUI Update
      button -> app + daemon + CLI return on the new version, shims still
      dispatch, Add/Remove Programs shows the new version, exactly one `*.exe.old`
      set remains and is cleaned on the next update cycle.
- [ ] **Data-safety (blocker #3):** after the self-update above, confirm the data
      dirs, the CA (CurrentUser Root store), the NRPT `.test` rule, and BOTH Run
      autostart values SURVIVE (the `$UpdateMode` guard held; the uninstall hook
      did NOT run on the update).
- [ ] Uninstall from Add/Remove Programs -> Run values, PATH entries, NRPT rule,
      CA, data dirs, and the install dir are all gone (a running PATH copy of
      `yerd.exe` may need a manual delete, as the uninstaller notes).

## Phase 6 - CI-only / frontend-only, not verifiable on the dev host

- [ ] NSIS bundle build (`npm run tauri build -- --config
      src-tauri/tauri.bundle-windows.conf.json`) - the config + `nsis/hooks.nsh`
      are written but only build in CI (blocked here: frontend `npm ci` deps are
      incomplete, and the bundler runs on `windows-latest`). Verify the emitted
      installer name (`Yerd_<ver>_x64-setup.exe`) and that the main app exe is
      `yerd-gui.exe` (the CI smoke asserts `Yerd.exe` with a `yerd-gui.exe`
      fallback - pin whichever the bundler actually emits on the first run).
- [x] The dormant `signCommand` seam note is out of
      `tauri.bundle-windows.conf.json` (strict JSON now); it lives in
      `docs/developer/building.md` instead, so the overlay parses whichever
      reader the CLI uses for `--config`.
- [ ] `build.yml` Windows leg + `release.yml` matrix/rename/sign/SHA256SUMS `.exe`
      cases are inert until a `windows-latest` run / a tag - verify on the first
      Windows RC.
- [ ] On the first Windows RC verify `latest.json`/CDN carry the `.exe` +
      `.exe.minisig` assets and the daemon's `CheckUpdate` sees them (no
      per-extension filter change was needed in `xtask`/`cdn-*`).
- [ ] Frontend (`apps/yerd-gui/src/**`) S4b edits (`usePlatform` +
      `usePlatform.test.ts`, `WelcomeView.vue`, `GeneralView.vue`,
      `EnvironmentCard.vue`, `DaemonDiagnosticsPanel.vue`) are UNVERIFIED here -
      run `npm ci && npm run test && npm run build` in `apps/yerd-gui` (node_modules
      is incomplete on the dev host per the Phase 1 TODO).
- [ ] Single-instance + autostart-plugin smoke on Windows is manual (launch twice
      -> one window; GUI "Start at login" writes HKCU Run `Yerd`; daemon toggle
      writes `Yerd Daemon`).
