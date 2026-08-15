# Phase 6 — Packaging, self-update, GUI polish: shippable Windows build

Working doc (not committed), same convention as PHASE1-5B_PLAN.md. Grounded in
the tree as of `eb14655` ("feat: add Windows PHP shims, PATH, and autostart");
`cargo check --workspace --all-targets` is green on this Windows machine.

---

## ⚠️ READ FIRST — blockers and human decisions

1. **DECISION #5 (packaging/signing) is still OPEN** — see the Decision section
   immediately below. Nothing in steps S5-S7 (bundling/CI) should be merged
   until the human picks. Recommended default: **NSIS-only, per-user
   (`installMode: currentUser`), unsigned with a wired-but-dormant signing
   hook**. Steps are written against that default.
2. **Frontend build/test is blocked in this environment.** `node_modules` is
   present but incomplete (`isomorphic-dompurify` / `dompurify` are missing, a
   pre-existing gap per TODO.md Phase 1). All `.vue`/`.ts` edits in S4 must be
   validated with `npm ci && npm run test && npm run build` in `apps/yerd-gui`
   on a machine with working npm — plan the edits here, verify there.
3. **One NSIS-template behaviour must be verified before the uninstall hook
   ships** (S6): that Tauri's generated uninstaller distinguishes a real
   uninstall from the silent uninstall an *upgrade* performs (the template's
   `$UpdateMode`, driven by the `/UPDATE` flag). If the guard doesn't hold, an
   app **update** would run `yerd uninstall --yes` and wipe the user's PHP
   installs, CA and NRPT rule. Mitigation is designed in S6; the verification
   is a one-time read of the vendored `installer.nsi` for the pinned
   tauri-bundler (2.6.2) + the clean-VM upgrade test in the DoD.
4. **Manual DoD on a clean Windows VM is unavoidable** (install → serve →
   update → uninstall). CI gets a silent-install smoke, but SmartScreen, UAC,
   WebView2 bootstrapper download and the upgrade-preserves-data check are
   VM-only.

---

## DECISION #5 — packaging & signing (for the human)

### (a) Installer format: NSIS-only vs NSIS + MSI/WiX

| | NSIS (Tauri default) | + MSI (WiX) |
|---|---|---|
| Per-user, no-UAC install | ✅ `installMode: currentUser` | ⚠️ WiX path is per-machine-oriented |
| Install/uninstall hooks | ✅ `installerHooks` .nsh macros | fragment XML, different mechanism |
| Silent update (`/S`) | ✅ our applier uses it | `msiexec /qn`, second applier path |
| Enterprise (GPO/Intune) | ❌ | ✅ |
| CI/QA surface | 1 artifact | 2 artifacts, 2 upgrade matrices |

**Recommendation: NSIS-only for MVP.** MSI goes to the post-MVP backlog (it is
already listed there in WINDOWS_PLAN). Nothing in the self-update wire design
below precludes adding an `Msi` artifact kind later (additive enum).

### (b) Install mode: per-user vs per-machine

**Recommendation: per-user (`currentUser`).** Everything Phase 1-5 built is
per-user: `%LOCALAPPDATA%\yerd` data dirs, HKCU Run autostart (locked decision
#3, option d), CurrentUser Root cert store (locked #1), HKCU `Environment\Path`
(Phase 5), SID-keyed per-user pipe. A per-machine install would put binaries in
`Program Files` (UAC at install *and* at every self-update) while all state
stays per-user anyway — worst of both. Per-user install dir is
`%LOCALAPPDATA%\Yerd` (Tauri NSIS default for `currentUser`), no UAC at
install, and self-update needs no elevation. Do **not** use Tauri's `"both"`
mode (doubles QA for no MVP gain).

### (c) Code signing: unsigned vs OV cert vs EV / Azure Trusted Signing

- **Unsigned (recommended for early access):** SmartScreen shows "Windows
  protected your PC" on first run of the downloaded installer (bypass: More
  info → Run anyway); Defender reputation is cold; some corporate
  AppLocker/WDAC setups will block outright. Zero cost, zero lead time, and
  **self-update integrity does not depend on Authenticode** — artifacts are
  SHA-256 + minisign verified against the key already embedded in
  `crates/yerd-update/src/artifact.rs` (`UPDATE_PUBLIC_KEY`), and the staged
  installer is written by the daemon without a Mark-of-the-Web, so the
  *in-app update path never hits SmartScreen* — only the first manual download
  does.
- **OV Authenticode cert** (~$100-400/yr, org validation, human must procure):
  reputation still starts cold; warnings fade only as downloads accrue.
- **EV cert or Azure Trusted Signing** (~$300+/yr or ~$10/mo Azure; requires a
  registered legal entity): immediate/fast SmartScreen reputation. CI wiring is
  small once the secret exists (Tauri `bundle.windows.signCommand`, or
  `certificateThumbprint` + signtool).
- **Recommendation:** ship early access **unsigned**, with the SmartScreen
  bypass documented (S8) and a `signCommand` seam left in the Windows bundle
  conf as a commented TODO so signing later is config + secret only. Decide
  signing **before public launch** (matches the WINDOWS_PLAN leaning). Not a
  launch blocker for early access.

**Supersession note:** the master-plan sentence "NSIS install hooks call the
Phase 5 `yerd-service-ctl` register op" is dead — Phase 5 (revised, option d)
uses per-user HKCU-Run autostart, enabled by the app at first-run/toggle via
`yerd_service_ctl::enable_at_login`. The installer registers **no service and
no Run key**; the uninstaller removes both Run values (S6).

---

## Verified current state (what this plan builds on)

- **Bundle configs** follow a base + per-OS overlay pattern:
  `apps/yerd-gui/src-tauri/tauri.conf.json` (base, `targets: ["deb","app"]`,
  icon list already includes `icons/icon.ico`) plus
  `tauri.bundle-macos.conf.json` / `tauri.bundle-linux.conf.json` /
  `tauri.bundle-linux-rpm.conf.json` overlays that add
  `externalBin: ["binaries/yerd","binaries/yerdd","binaries/yerd-helper"]`.
  There is **no Windows overlay yet**. Tauri is 2.11.2 / tauri-bundler via
  tauri-build 2.6.2 (Cargo.lock) — NSIS target, `installMode: currentUser`,
  `webviewInstallMode: downloadBootstrapper`, and `installerHooks` are all
  supported at this version.
- **CI:** `.github/workflows/ci.yml` already runs `windows-latest` tests. The
  release build (`.github/workflows/build.yml`, reusable) has bespoke legs
  keyed on `matrix.kind` (`gui` / `arch` / `fedora`); each GUI leg natively
  builds the 3 bins, stages them as `binaries/<name>-<triple>` sidecars, runs
  `npm run tauri build -- --config src-tauri/tauri.bundle-<os>.conf.json`, then
  asserts bundle contents. `release.yml` passes the target matrix, renames
  assets to `Yerd_<OS>_<Arch>_v<ver>.<ext>`, minisigns
  `*.app.tar.gz|*.deb|*.pkg.tar.zst|*.rpm`, writes SHA256SUMS, publishes.
  **Every one of those glob lists needs a `.exe` case** (S7).
- **Self-update, daemon side** (`bin/yerdd/src/self_update.rs`): fetches the
  signed `latest.json` from the CDN, `select_asset(release,
  Platform::current(), PkgFormat::current())`, downloads + SHA256 + minisign
  verifies, stages to `{cache}/update/<asset>` + `<asset>.minisig`, replies
  `Response::Staged { path, version, kind }`. The `ArtifactKind →
  StagedArtifact` mapping at `self_update.rs:433` is an **exhaustive match** —
  adding an `ArtifactKind` variant will not compile without touching it (this
  pins the step boundaries below).
- **Self-update, applier side** (`bin/yerd/src/apply.rs`): `run()` re-verifies
  minisign then dispatches on `StagedArtifact`; Windows currently falls to
  `_ => Err("unknown staged artifact kind")`. The Unix flows are macOS
  `tar -xpf` + `swap_bundle` rename-aside/rename-in, and Linux
  `pkexec dpkg/pacman/rpm`. So **yes, the current apply path assumes
  `tar -xpf`/package managers; nothing Windows-shaped exists yet** — but the
  rename-aside/rename-in + rollback shape (`swap_bundle`, unit-tested on temp
  dirs) is exactly the primitive the Windows staged-rename needs.
- **`is_windows_*` matchers deferred from Phase 1**: confirmed.
  `crates/yerd-update/src/artifact.rs` has `Platform::Windows{X86_64,Aarch64}`
  variants whose `select_asset` arm returns `NoArtifactForPlatform`, with a
  test `windows_has_no_selfupdate_artifact_yet` pinning that. No `is_windows_*`
  matcher exists. `yerd_ipc::StagedArtifact`
  (`crates/yerd-ipc/src/update.rs:42`) is `#[non_exhaustive]`,
  snake_case-serialized, wire-pinned by
  `crates/yerd-ipc/tests/wire_stability.rs::staged_artifact_each_variant_byte_shape`
  — the Windows kind is a clean **additive** change.
- **Restart (Phase 5)**: `bin/yerdd/src/main.rs::restart_in_place` (Windows)
  spawns `current_exe()` detached with `RESTART_HANDOFF_ENV` and exits — so a
  daemon-initiated restart re-executes **whatever path the running image has**.
  Caveat: on Windows, `current_exe()` follows a rename of the running image, so
  after a rename-aside the daemon would respawn `yerdd.exe.old`. Therefore the
  applier, not the daemon, owns the update restart: it uses
  `yerd_service_ctl::ServiceCtl` (Windows backend exists: `taskkill` stop →
  `tasklist` wait → hidden detached `yerdd serve` start; `restart()` =
  stop→wait→start), exactly as the Phase 5B handoff note in TODO.md prescribes.
- **`.cmd` shims**: `crates/yerd-platform/src/pure/win_shim.rs::wrapper_body`
  embeds the **absolute** `yerd.exe` path (not `%~dp0`) passed by the daemon's
  `yerd_sibling()` (sibling of the running `yerdd`, i.e. the install dir). So
  shims survive self-update **iff the install-dir path is stable** — it is
  (`%LOCALAPPDATA%\Yerd`), so **no shim rewrite on update**, as planned. (The
  master-plan text says "relative path"; the reality is "stable absolute path"
  — same conclusion.)
- **PATH copy**: `bin/yerd/src/path_cmd.rs` (windows mod) copies `yerd.exe`
  into `%LOCALAPPDATA%\Programs\yerd\bin`, puts that dir + the `{data}\bin`
  shim dir on the HKCU PATH, and **already stages `yerd.exe.new` beside a
  locked destination with a "restart to finish" note** — Phase 6 must finish
  that staged swap (S3).
- **Uninstall**: `bin/yerd/src/uninstall.rs::windows_impl` removes NRPT (via
  elevated helper), CA (CurrentUser store), the `Yerd Daemon` Run value
  (`yerd_service_ctl::disable_at_login`), kills `yerdd.exe`, removes PATH
  entries + dirs, and leaves a residue note that the binaries themselves need
  the Phase 6 uninstaller. Ready to be the uninstaller hook's workhorse.
- **GUI gaps (all confirmed in source)**:
  - `daemon.rs::resolve_binary` joins bare names (`"yerdd"`, `"yerd"`) — no
    `.exe`, so **binary resolution always fails on Windows** (breaks daemon
    start, apply_update, elevate). Must fix (S4a).
  - `autostart.rs::plan_start` / `daemon_stop` / `daemon_set_login` /
    `service_registered` and `daemon.rs::install_cli_to_path` are
    `not(linux|macos)` error stubs; `src-tauri/Cargo.toml` does not yet depend
    on `yerd-service-ctl`.
  - `commands.rs::spawn_applier` is `#[cfg(unix)]`; the non-unix stub errors.
  - `elevate.rs` (GUI) has no Windows arm (Phase 4 TODO deferred here).
  - Frontend: `usePlatform.supportsPathInstall` excludes windows;
    `EnvironmentCard.vue` `canElevate` is linux|macos and its copy says
    `sudo yerd unelevate trust`; `DaemonDiagnosticsPanel` says "socket".
- **Release-manifest/CDN**: `xtask` cdn-reconcile and `cdn-sync.yml` have no
  per-extension filters (verified by grep), so a new `.exe` release asset flows
  into `latest.json`/CDN automatically. One-time verify on the first RC.

---

## The Windows self-update design (hard area #3)

### Artifact strategy: ONE Windows artifact — the NSIS installer

Publish a single Windows asset per release, `Yerd_Windows_x86_64_v<ver>.exe`
(+ `.minisig`, + SHA256SUMS line). The applier applies it the same way for both
invocation modes (CLI in-process, GUI detached): **rename-aside the in-use
exes, run the installer silently, restart**.

Why not the master plan's split (zip staged-rename for CLI/daemon + installer
for GUI)? Two artifacts to sign/publish/select, and a zip-applied update leaves
the NSIS uninstall registry (DisplayVersion, file list) stale — Add/Remove
Programs lies and a later uninstall can miss files. The rename-aside step keeps
the "staged rename of a running exe" mechanics the plan wanted, but the
installer remains the single source of on-disk truth. The `StagedArtifact` enum
stays open for a `Zip` kind later if a daemon-only fast path is ever wanted.

### Flow (applier = `yerd.exe` in hidden `YERD_APPLY_UPDATE` mode, or in-process for CLI)

1. `reverify(staged)` — unchanged, cross-platform already.
2. Resolve the install dir: dir of `current_exe()` if it contains `yerdd.exe`,
   else `%LOCALAPPDATA%\Yerd` if that contains `yerdd.exe`, else error
   ("not an installed copy — reinstall from the installer"). Pure helper
   `find_install_dir(candidates...)`, table-tested.
3. Delete any `*.exe.old` left from the previous cycle (this is the "keep
   `.old` one cycle" rollback window closing).
4. Stop the daemon: `ServiceCtl::new(install_dir\yerdd.exe).stop()` +
   `wait_for_exit` (both exist in `yerd-service-ctl`).
5. Rename-aside every in-use image in the install dir:
   `Yerd.exe`, `yerd.exe`, `yerdd.exe`, `yerd-helper.exe` → `<name>.exe.old`
   (Windows allows renaming a running exe; this is what makes updating the
   running `yerd.exe` possible). Implemented as a cross-platform-testable
   `rename_aside(dir, names) -> Result<RenamedSet>` with a `rollback()` that
   renames back — mirror of `swap_bundle`'s rollback discipline.
6. Run the staged installer: spawn `<staged>.exe` with args `/S /UPDATE`,
   **wait** for exit. `/S` = silent; `/UPDATE` tells Tauri's NSIS template this
   is an update (preserves app data paths / update-mode semantics — same flag
   Tauri's own updater passes). Non-zero exit → `rollback()` the renames,
   restart daemon, report error. Defender may briefly lock the staged file:
   retry the spawn once after ~200ms (Phase 2 precedent).
7. Refresh the PATH copy: if `%LOCALAPPDATA%\Programs\yerd\bin\yerd.exe`
   exists, rename it aside (`yerd.exe.old`) and copy the new
   `install_dir\yerd.exe` in (finishes the `yerd.exe.new` staging story from
   Phase 5's `path_cmd`; also delete a stale `yerd.exe.new` if present).
8. Restart: `ServiceCtl::new(install_dir\yerdd.exe).start()`; if
   `relaunch_gui`, spawn `install_dir\Yerd.exe` detached
   (`CREATE_NO_WINDOW` not needed — it's a GUI-subsystem exe; plain detached
   spawn).
9. `.cmd` shims: untouched (stable absolute path, see above). The daemon's
   startup `reconcile_shims` also re-verifies them for free.

No MOTW/SmartScreen on step 6: the daemon writes the staged installer with
`tokio::fs::write` (no `Zone.Identifier` ADS), and `CreateProcess` doesn't
consult SmartScreen anyway.

### The additive wire change (FLAGGED — IPC contract)

This is the Phase-1-deferred change, landing now, strictly additively:

| Surface | Change | Wire pin |
|---|---|---|
| `crates/yerd-update/src/artifact.rs` | `ArtifactKind::NsisExe`; `is_windows_x86_64_artifact(name)` = `ends_with(".exe") && (x86_64 \| x64 \| amd64)` (case-insensitive token match like the arm64 matchers, and it must NOT match `.exe.minisig` — suffix check handles that); `select_asset` arm `(Platform::WindowsX86_64, _) => (NsisExe, is_windows_x86_64_artifact)` (format ignored, like macOS). `WindowsAarch64` **stays** `NoArtifactForPlatform` (no arm64 runner/artifact in MVP — mirrors Intel macOS). | unit tests in the same file (S1) |
| `crates/yerd-ipc/src/update.rs` | `StagedArtifact::NsisExe` variant (docs: "Windows NSIS installer - the applier runs it silently (`/S /UPDATE`)"). Serializes as `"nsis_exe"`. `#[non_exhaustive]` means old peers fail closed on decode — acceptable: only a new daemon ever *sends* it, and a new daemon ships with a new CLI/GUI in the same install. | **`wire_stability.rs::staged_artifact_each_variant_byte_shape` gains the `NsisExe` ⇄ `"nsis_exe"` round-trip** (this is the wire-pin test; treat any failure as a contract alarm) |
| `bin/yerdd/src/self_update.rs:433` | `ArtifactKind::NsisExe => StagedArtifact::NsisExe` (exhaustive match forces this in the same commit as the yerd-update change) | covered by the two pins above |
| `bin/yerd/src/apply.rs` | env parser `Ok("nsis_exe") => StagedArtifact::NsisExe`; `run()` arm `StagedArtifact::NsisExe => apply_windows(staged, relaunch_gui)` (+ non-windows stub mirroring `apply_macos`'s cross-platform stub) | unit test: parser accepts `nsis_exe`, rejects junk (extend the existing env-contract tests) |
| `apps/yerd-gui/src-tauri/src/commands.rs` | `kind_str`: `StagedArtifact::NsisExe => "nsis_exe"` | the "must stay in sync" doc comment already binds it to the apply.rs parser |

No other `Request`/`Response` shape changes. `Response::Staged` is unchanged.

---

## Ordered implementation checklist

Each step leaves `cargo check/clippy/test --workspace` green on Windows +
Unix. Frontend-only steps (S4b) and bundle/CI steps (S5-S7) are verifiable only
with npm deps / in CI — flagged inline.

### S1. Wire + selection: the Windows artifact kind (one commit, all five files)

- `crates/yerd-update/src/artifact.rs`: `ArtifactKind::NsisExe`,
  `is_windows_x86_64_artifact`, `select_asset` arm; move `WindowsX86_64` out of
  the no-artifact arm (leave `WindowsAarch64` + `MacOsX86_64` + `Unsupported`
  there). Update `Platform` doc comments ("no artifact" claim now stale).
- `crates/yerd-ipc/src/update.rs`: `StagedArtifact::NsisExe`.
- `bin/yerdd/src/self_update.rs`: mapping arm (compiler-forced).
- `bin/yerd/src/apply.rs`: env-parser arm + `run()` dispatch arm + a
  `#[cfg(not(windows))] fn apply_windows(..) -> Err("a Windows installer cannot
  be installed on this platform")` stub; the real Windows body lands in S2 —
  to keep S1 compiling on Windows too, the `#[cfg(windows)]` body can land
  here as `Err("not yet implemented")` **only if S2 is a separate commit the
  same day**; otherwise fold S1+S2 (preferred: fold — "never half-flip").
- `apps/yerd-gui/src-tauri/src/commands.rs`: `kind_str` arm.
- **Tests (same commit):**
  - artifact.rs: `selects_windows_x86_64_nsis_installer` (asset
    `Yerd_Windows_x86_64_v2-0-5.exe` + `.minisig` + SHA256SUMS → kind
    `NsisExe`, sig `.exe.minisig`); matcher disjointness (an `.exe` never
    matches deb/pacman/rpm/mac matchers and vice versa; `.exe.minisig` not
    selected as artifact); `windows_aarch64_still_has_no_artifact`; **rewrite**
    `windows_has_no_selfupdate_artifact_yet` → x86_64 with an *empty* release
    still errors `NoArtifactForPlatform` (name it
    `windows_x86_64_errors_when_release_has_no_exe`).
  - wire_stability.rs: `nsis_exe` round-trip (FLAGGED wire pin, above).

### S2. Windows applier (`bin/yerd/src/apply.rs`) — the risk center

- New `#[cfg(windows)] mod` (or inline fns) implementing the 9-step flow above:
  - `find_install_dir()` — pure candidate-scan helper + table tests (runs on
    all OSes: it takes `&[PathBuf]`-style injected candidates or a probe
    closure).
  - `rename_aside` / `RenamedSet::rollback` — cross-platform functions (like
    `swap_bundle`), unit-tested on temp dirs on every OS: happy path, partial
    failure mid-set rolls back the already-renamed ones, missing source file is
    skipped not fatal (`yerd-helper.exe` may be absent in a dev tree).
  - `apply_windows(staged, relaunch_gui)`: wires steps 2-8;
    `sibling_yerdd()`-analogue via install dir; installer spawn with
    `Command::new(staged).args(["/S", "/UPDATE"]).status()` + one bounded
    retry on spawn failure; `relaunch_gui_app()` Windows impl spawning
    `install_dir\Yerd.exe` (fallback try `yerd-gui.exe` — see S5 assert note).
  - Programs-bin copy refresh (step 7) — reuse the `path_cmd::windows` logic
    by lifting `programs_bin()` or duplicating the 6 lines (keep `path_cmd`
    private; duplication is fine and keeps modules decoupled — match repo
    style either way).
- `bin/yerd/src/uninstall.rs`: drop/soften the residue line claiming binaries
  need manual removal "(the Phase 6 installer's uninstaller will handle this)"
  → now true; keep the note only for non-installer (cargo/dev) layouts.
- GUI `commands.rs::spawn_applier`: add `#[cfg(windows)]` variant — same env
  vars, `Stdio::null()`, `creation_flags(CREATE_NO_WINDOW |
  CREATE_NEW_PROCESS_GROUP)` (constants exist in this tree twice already;
  copy locally), no `process_group(0)`, never `YERD_APPLY_GUI_OWNS_DAEMON`.
  Narrow the `#[cfg(not(unix))]` stub to `#[cfg(not(any(unix, windows)))]`.
- **Tests:** the pure helpers above; `gui_owns_daemon`/env-parse tests extend;
  existing `swap_bundle` tests untouched. Live installer execution is a CI
  smoke (S7) + VM DoD, not a unit test.

### S3. `path_cmd` staged-swap finish (small)

- `bin/yerd/src/path_cmd.rs::windows::copy_self_into_programs`: on startup of
  any `yerd path install`/`ensure_installed_after_tool` call, first complete a
  pending swap: if `yerd.exe.new` exists and `yerd.exe` is replaceable, rename
  `yerd.exe`→`.old`, rename `.new`→`yerd.exe`, delete stale `.old` from a
  prior cycle. (~15 lines; the "restart yerd to finish" note becomes true.)
- **Tests:** pure rename-sequence test on temp dir (all OSes).

### S4a. GUI Rust glue (compiles via cargo on any OS; no npm needed)

- `apps/yerd-gui/src-tauri/Cargo.toml`:
  `[target.'cfg(windows)'.dependencies] yerd-service-ctl = { path = ... }`
  (mirrors `bin/yerdd`'s cfg-gated dep).
- `daemon.rs::resolve_binary`: append `.exe` on Windows — one small
  `fn exe_name(name: &str) -> String` (`format!("{name}.exe")` under
  cfg(windows), passthrough otherwise), used for the sibling probe and
  `search_dirs` join. **This unblocks daemon start, apply_update, elevate and
  cli-path on Windows** (today `is_file()` never matches). Unit-test the pure
  name mapping.
- `autostart.rs` Windows arms (per the TODO.md 5B handoff):
  - `plan_start`: one `StartStep { Starting, START_BUDGET, run:
    ServiceCtl::new(resolve_yerdd()?).start() }` (spawn-detached backend
    exists).
  - `daemon_stop`: `ServiceCtl::stop()` (taskkill; Job Objects reap children).
  - `daemon_set_login(on)`: `enable_at_login(&yerdd)` / `disable_at_login()`.
  - `get_autostart`/`daemon_login_enabled` probe: `autostart_enabled()`.
  - `service_registered()` (drives `is_set_up`): `autostart_enabled()` — the
    Run value is the Windows analogue of "unit file exists".
- `daemon.rs::install_cli_to_path` / `cli_path_status` /
  `remove_cli_from_path` Windows arms: no symlink — spawn the resolved sibling
  `yerd.exe` with `["path", "install"]` (does copy + PATH + broadcast);
  status = `programs_bin\yerd.exe` exists; remove = spawn
  `["path", "uninstall"]`. (Keeps the GUI a thin client of the CLI, same as
  Unix which shells to `yerd path install`.)
- `elevate.rs` (GUI) Windows arm (Phase 4 handoff): spawn the sibling
  `yerd.exe elevate <target...>` **unelevated** with `CREATE_NO_WINDOW`,
  capture stderr for the error path — the CLI's helper raises UAC per-op
  itself. `run_many` joins targets as multiple args like Unix.
- **Tests:** pure name/argv helpers; the rest is thin spawning (matches the
  existing module's test level).

### S4b. Frontend polish (`apps/yerd-gui/src/**`) — ⚠ needs `npm ci` to verify

Audit result (grep for sudo/Terminal.app/bashrc/`/usr/local`/`.sock`): the
Unix-isms live in exactly these files.

- `composables/usePlatform.ts`: `supportsPathInstall` → include `"windows"`.
  Update `usePlatform.test.ts` (windows case currently asserts `false`).
- `views/GeneralView.vue`: "Terminal CLI" card now shows on Windows via the
  flag; copy tweak behind `isWindows` — "run \`yerd\` in a **new** terminal
  window" already fits; change the install description's "shell PATH" →
  "PATH" (cosmetic, optional).
- `views/WelcomeView.vue`: step-2 CLI install un-gates via the same flag;
  update the stale comment ("not yet wired up on Windows").
- `components/EnvironmentCard.vue`: `canElevate` → include `"windows"`; make
  the three `sudo yerd unelevate trust` strings and the untrust modal body
  platform-aware (Windows: no system-wide-trust concept — the CA lives only in
  the CurrentUser store, so drop the residual-trust caveat text; the
  `systemTrustRemains` machinery is already macOS-gated). The `ComingSoon`
  "use \`yerd elevate\` in a terminal" fallback then only shows on genuinely
  unsupported platforms.
- `components/DaemonDiagnosticsPanel.vue`: label `socket` →
  platform-neutral "endpoint" (or gate "pipe" on `isWindows`). Cosmetic;
  include since it's one line.
- Windows-appropriate empty-state/hints: search found **no** other
  bashrc/Terminal.app/`/usr/local` strings in `src/**` — the audit is done,
  don't invent more.
- **Verification (deferred to a machine with deps):** `npm ci`,
  `npm run test`, `npm run build`. Single-instance + autostart plugin smoke
  on Windows is a manual item: launch app twice → one window; GUI "Start at
  login" toggle writes HKCU Run `Yerd`; daemon toggle writes `Yerd Daemon`.

### S5. Tauri Windows bundle overlay + NSIS hooks

- New `apps/yerd-gui/src-tauri/tauri.bundle-windows.conf.json` (matches the
  existing overlay pattern):

  ```json
  {
    "$schema": "https://schema.tauri.app/config/2",
    "bundle": {
      "targets": ["nsis"],
      "externalBin": ["binaries/yerd", "binaries/yerdd", "binaries/yerd-helper"],
      "windows": {
        "webviewInstallMode": { "type": "downloadBootstrapper", "silent": true },
        "nsis": {
          "installMode": "currentUser",
          "installerHooks": "nsis/hooks.nsh"
        }
      }
    }
  }
  ```

  Notes: `targets` must be overridden (base `["deb","app"]` yields nothing on
  Windows); `icon.ico` already flows from the base icon list; leave a
  commented `"signCommand"` TODO seam per decision (c). Tauri strips the
  `-rc.N` pre-release for the numeric VIProductVersion automatically — verify
  once in CI output on the first RC tag.
- New `apps/yerd-gui/src-tauri/nsis/hooks.nsh`:
  - `NSIS_HOOK_POSTINSTALL`: `nsExec::ExecToStack '"$INSTDIR\yerd.exe" path
    install'` (per-user, no console window, idempotent; puts `yerd` +
    shim dir on the user PATH — the Phase-5 `path_cmd`). **No service/Run-key
    registration** (option-d reality; the app enables autostart itself).
  - `NSIS_HOOK_PREUNINSTALL`: **guarded** so it runs only on a real uninstall,
    not the upgrade-time silent uninstall — `${If} $UpdateMode <> 1` (verify
    the variable against tauri-bundler 2.6.2's vendored `installer.nsi`; see
    blocker #3): `nsExec::ExecToStack '"$INSTDIR\yerd.exe" uninstall --yes'`
    (removes NRPT via one UAC, CA, daemon Run value, PATH entries, data dirs,
    kills yerdd; already non-interactive under `--yes`), then belt-and-braces
    `DeleteRegValue HKCU "...CurrentVersion\Run" "Yerd"` and `"Yerd Daemon"`
    (the GUI's plugin-written value is otherwise orphaned, per TODO.md).
  - If `$UpdateMode` turns out not to exist/behave in 2.6.2's template:
    fallback design = keep only the registry `DeleteRegValue` lines in the
    uninstall hook and move the `yerd uninstall --yes` call behind an
    uninstaller UI page/flag — do NOT ship the unguarded call.
- Local build verification on this machine is possible **except** the
  `beforeBuildCommand` (`npm run build`) — blocked per blocker #2. Once npm
  deps exist: `npm run tauri build -- --config
  src-tauri/tauri.bundle-windows.conf.json` after staging
  `binaries/{yerd,yerdd,yerd-helper}-x86_64-pc-windows-msvc.exe`.

### S6-S7. CI: build leg + publish plumbing (verifiable only in CI)

- `.github/workflows/build.yml` — new `windows` kind, mirroring the GUI legs'
  shape (checkout with `persist-credentials: false`, setup-node + npm cache,
  rust-cache, `npm ci`), every `run:` step with explicit `shell: bash`
  (windows-latest defaults to pwsh; the repo's step style is bash):
  1. *Stage embedded binaries*: `cargo build --release -p yerd -p yerdd -p
     yerd-helper`; pkg-format guard asserts `deb` (the default; keeps the
     symmetric-guard pattern honest — a `pacman`/`rpm` leak here would be a
     matrix bug); `cp target/release/$b.exe
     apps/yerd-gui/src-tauri/binaries/$b-x86_64-pc-windows-msvc.exe`. Native
     build on `windows-latest` — **no cross-compilation anywhere**; Tauri's
     bundler downloads NSIS itself on the runner (no choco prereq), and
     bundling needs no WebView2 runtime (that's install-time,
     `downloadBootstrapper`).
  2. *Build NSIS bundle*: `npm run tauri build -- --config
     src-tauri/tauri.bundle-windows.conf.json` → 
     `apps/yerd-gui/src-tauri/target/release/bundle/nsis/Yerd_<ver>_x64-setup.exe`.
  3. *Assert + smoke* (the Windows analogue of the deb-contents assert): run
     the installer silently `./Yerd_*-setup.exe /S`, bounded-wait for the NSIS
     child to finish, then assert `%LOCALAPPDATA%\Yerd\{yerd.exe, yerdd.exe,
     yerd-helper.exe}` exist and the main app exe exists (assert `Yerd.exe`,
     falling back to `yerd-gui.exe` — pin whichever the bundler actually
     emits on the first run, then hard-assert it); assert `yerd.exe --version`
     exits < 128; assert the postinstall hook ran (Programs\yerd\bin\yerd.exe
     exists / HKCU PATH contains the entries). Advisory (non-blocking): run
     `%LOCALAPPDATA%\Yerd\uninstall.exe /S` and check the Run values are gone
     — UAC-dependent bits (NRPT) are expected residue on a runner.
  4. *Upload*: extend the upload-artifact `path:` union with
     `apps/yerd-gui/src-tauri/target/release/bundle/nsis/*.exe` and
     `target/release/bundle/nsis/*.exe`.
- `.github/workflows/release.yml`:
  - matrix: add
    `{"id":"windows-x86_64","runner":"windows-latest","container":"","kind":"windows"}`.
  - *Flatten + rename*: add `-o -name '*.exe'` to the `find`, and a case arm
    `*.exe) ext="exe"; os="Windows" ;;` plus `*x64*` in the arch-token match
    (Tauri names it `..._x64-setup.exe`) → `Yerd_Windows_x86_64_v<ver>.exe`.
  - *Sign*: add `*.exe` to the minisign loop and the trust-anchor verify loop.
    **No legacy `.sig` copy** for `.exe` (no pre-existing Windows clients).
  - *SHA256SUMS*: add `-o -name '*.exe'`.
- `build-cdn.yml` / `cdn-sync.yml` / `xtask cdn-reconcile-plan`: no code change
  expected (no extension filters found); verify on the first Windows RC that
  `latest.json` carries the `.exe` + `.minisig` assets and the daemon's
  `CheckUpdate` sees it.
- `.github/workflows/ci.yml`: no change — the windows-latest test leg already
  runs all new unit tests.

### S8. Docs

- `README.md`: platforms badge → `macOS · Linux · Windows (early access)`;
  intro copy + comparison-table `Windows support` cell for Yerd → ✅ (early
  access); Installation table row: Windows `.exe` installer download +
  SmartScreen "More info → Run anyway" note (per decision (c)).
- `docs/guide/getting-started.md` (+ a short `docs/guide/windows.md` if the
  nav wants one page): install steps, then **known MVP limitations**, verbatim
  from the phase notes: CurrentUser-only cert trust (one-time confirmation
  dialog; Edge/Chrome covered, **Firefox needs manual trust** — no NSS
  auto-trust); ACL hardening of CA key/runtime dir is a tracked TODO; no
  system metrics in the GUI; brief console flash at logon (cosmetic,
  documented escape hatch in TODO.md); one concurrent PHP request per version
  (php-cgi, post-MVP worker pool); unsigned installer SmartScreen warning;
  self-update replaces the installed app via the silent installer.
- `.github/copilot-instructions.md` + `CLAUDE.md`: update the "Windows support
  is planned / adapters don't exist" sentences to the shipped-subset reality
  (keep the cross-platform-discipline text; it already describes the growing
  `Windows*` set).
- `docs/developer/building.md`: Windows build prerequisites (MSVC toolchain,
  npm; NSIS auto-downloaded by tauri-bundler) + the overlay-config invocation.

---

## Ordering constraints & step-compile summary

S1(+S2 folded) → S3 → S4a → S4b → S5 → S6/S7 → S8. S1 must be atomic across
its five files (exhaustive-match + wire-pin coupling). S4a is independent of
S2 but both precede any real Windows GUI update test. S5 precedes S6. S7's
release.yml edits are inert until a tag, so they can land with S6. Docs last.
Nothing here touches macOS/Linux behaviour except shared-file additive arms;
the existing Unix tests are the regression net, plus `ci.yml`'s three-OS run.

**New pinned deps: none.** (`winreg`, `interprocess`, `zip`, `yerd-service-ctl`
wiring all exist; the GUI gains only a path-dep on the in-workspace
`yerd-service-ctl` behind `cfg(windows)`. The NSIS hooks file is not a dep.)

## Explicitly out of scope (post-MVP backlog, unchanged from the master plan)

MSI/WiX; code signing execution (seam only); LocalMachine cert store; Windows
Firefox/NSS certutil trust; `secure_fs.rs` ACL enforcement; `SystemMetrics`;
winget/scoop manifests; arm64 Windows artifact; node/bun tool shims on
Windows; graceful IPC `Shutdown` request.

## Definition of done

- CI: green three-OS test matrix; the windows build leg produces
  `Yerd_Windows_x86_64_v<ver>.exe`, silent-install smoke passes, publish
  attaches `.exe` + `.exe.minisig` + SHA256SUMS entry and the trust-anchor
  gate verifies it.
- Wire: `staged_artifact` pin test covers `nsis_exe`; all matcher tests green.
- Clean Windows VM (manual): download installer → SmartScreen bypass → install
  (no UAC) → GUI launches → first-run enables daemon autostart (HKCU Run) →
  `yerd`/`php` resolve in a fresh shell → create site → `https://x.test`
  green padlock → `php -v` per-site dispatch → seed an older version, run
  `yerd update --yes` AND the GUI Update button → app+daemon+CLI come back on
  the new version, shims still dispatch, Add/Remove Programs shows the new
  version, exactly one `.old` set remains and is cleaned next cycle →
  **upgrade did NOT delete data dirs/CA/NRPT** (blocker-#3 guard) → uninstall
  from Add/Remove Programs → Run values, PATH entries, NRPT, CA, dirs, and
  install dir all gone.
- macOS/Linux release flow byte-identical (rename/sign loops only gained
  `.exe` cases).
