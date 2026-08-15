# Phase 1 Implementation Plan — Windows Foundations (compile, resolve, locate, talk)

Working doc (not committed). Companion to `WINDOWS_PLAN.md` Phase 1. Everything below
was verified against the actual code on 2026-08-02, including live `cargo check` runs
on this Windows machine.

---

## 0. Ground truth (verified, not assumed)

### 0.1 The workspace does NOT compile on Windows today

`cargo check` per package on Windows 11, toolchain from `rust-toolchain.toml`:

| Package | Result |
|---|---|
| every library crate (`yerd-core` … `yerd-update`, `xtask`) | **compiles clean** |
| `bin/yerdd` | **5 errors** — `ActivePaths::new()` (`startup.rs:105`), `ActivePortBinder::new()` (`startup.rs:163`, `startup.rs:223`, `services.rs:46`, `services.rs:1034`) |
| `bin/yerd` | **2 errors** — `ActivePaths::new()` (`lib.rs:932`, `path_cmd.rs:24`) |
| `bin/yerd-helper` | **5 errors** — `exec.rs:11,12,14,16,17`: `ops::ca::{install_ca,uninstall_ca}`, `ops::resolver::{install_resolver,uninstall_resolver}`, `ops::setcap::setcap` do not exist on non-Linux/macOS (the op modules only ship `#[cfg(target_os = "linux")]` / `#[cfg(target_os = "macos")]` bodies) |
| `apps/yerd-gui/src-tauri` | **8 errors** — `ActivePaths::new()` ×7 (`autostart.rs:165,225`, `daemon.rs:135,454`, `logging.rs:114,188,378`) and un-gated `libc::kill` in `daemon.rs:311` (`fn sigterm`) |

Root cause of the `new()` errors: `crates/yerd-platform/src/os/unsupported.rs` gives
`UnsupportedTerminalLauncher`, `UnsupportedTrustStore`, `UnsupportedResolverInstaller`,
`UnsupportedSystemMetrics`, `UnsupportedPortRedirector` a `pub const fn new()`, but
**`UnsupportedPaths` (line 42) and `UnsupportedPortBinder` (line 137) have none** — the
callers use `ActivePaths::new()` / `ActivePortBinder::new()`. So Item 1 below is a hard
precondition for everything else in this phase.

### 0.2 `interprocess` 2.4.2 CAN set a pipe security descriptor (verified in source)

From the published 2.4.2 crate source:

- `interprocess::os::windows::local_socket::ListenerOptionsExt` —
  `fn security_descriptor(self, sd: SecurityDescriptor) -> Self` (applies to the same
  `ListenerOptions` the daemon already uses).
- `interprocess::os::windows::security_descriptor::SecurityDescriptor::deserialize(sdsf: &U16CStr) -> io::Result<Self>` —
  safe public API that parses an SDDL string (wraps
  `ConvertStringSecurityDescriptorToSecurityDescriptorW`; the `unsafe` lives inside
  `interprocess`, not our code).
- `U16CStr`/`U16CString` come from the `widestring` crate (an existing transitive dep of
  `interprocess`, already in `Cargo.lock` at 1.x); interprocess does **not** re-export it,
  so `yerd`/`yerdd`/`yerd-gui` need `widestring` as a direct `cfg(windows)` dependency.

**Consequence: no raw Win32 `CreateNamedPipe` fallback is needed, and no `unsafe` is
required anywhere in Phase 1 for the listener.** (Escalation contingencies in §10.)

### 0.3 Sibling artifact contracts already define the Windows tokens

- `yerd-services` README: `os ∈ linux | macos | windows`, **Windows is x86_64 only**,
  filenames `mysql-8.4.9-windows-x86_64.tar.gz` — exactly what
  `artifact_filename()` will produce once `Os::Windows` exists (`as_str() == "windows"`).
- `yerd-php` README: Windows builds are repackaged windows.php.net bundles
  (`php.exe` + `php-cgi.exe`). **Contract note for Phase 2, not Phase 1:** the consumer's
  `BinaryKind::archive_member()` (`"php"`, `"php-fpm"`) and `install_segments()`
  (`bin/php`, `sbin/php-fpm`) do not match a `.exe`/cgi layout. Phase 1 only adds the
  enum arm; `resolve_from_listing` consumes manifest `file` fields verbatim so nothing
  breaks — but confirm the published `php.json` Windows rows (cli/fpm entry shape)
  before starting Phase 2, per CLAUDE.md's cross-repo rule.

### 0.4 `hostPlatform` needs no daemon change

`apps/yerd-gui/src-tauri/src/commands.rs:724`:

```rust
#[tauri::command]
pub fn host_platform() -> &'static str {
    std::env::consts::OS
}
```

It is a **host-side Tauri command**, not daemon IPC, and already returns `"windows"` on
Windows. Phase 1's "platform identity" work is therefore frontend-only (Item 7).

---

## Implementation checklist

Items are ordered; each ends at a compiling workspace (the "never half-flip" rule).
Within an item, sub-steps land as one change.

---

### Item 1 — Compile floor: make `cargo check --workspace` green on Windows

No behaviour change on any OS. Everything else in Phase 1 builds on this.

**1a. `crates/yerd-platform/src/os/unsupported.rs`**

- Add to `UnsupportedPaths` (after line 42) and `UnsupportedPortBinder` (after line 137)
  the same constructor the five sibling stubs already have:

  ```rust
  impl UnsupportedPaths {
      /// Construct.
      #[must_use]
      pub const fn new() -> Self { Self }
  }
  ```

  (Mirror check: `LinuxPaths`/`MacosPaths`/`LinuxPortBinder`/`MacosPortBinder` all have
  `new()`; only the stub lacked it — this is a pure omission fix.)

**1b. `bin/yerd-helper/src/ops/{ca.rs,resolver.rs,setcap.rs}`**

- Current state: each op has `#[cfg(target_os = "linux")]` and `#[cfg(target_os = "macos")]`
  bodies only (e.g. `setcap.rs:11` and `setcap.rs:24`); `exec.rs::dispatch` calls them
  unconditionally.
- Add a `#[cfg(not(any(target_os = "linux", target_os = "macos")))]` stub for each of the
  five functions returning `HelperError::Unsupported { operation: ops::<TAG> }`, exactly
  mirroring the existing macOS `setcap` stub (`setcap.rs:24-29`). Tags:
  `ops::INSTALL_CA`, `ops::UNINSTALL_CA`, `ops::INSTALL_RESOLVER`,
  `ops::UNINSTALL_RESOLVER`, `ops::SETCAP`.
- Also clean the "unused import" warnings this exposes (`setcap.rs:3,5`,
  `resolver.rs:11`) by moving the imports under the cfg that uses them —
  `-D warnings` on the new CI leg will otherwise fail.
- The real Windows privilege model (token elevation, exit-78 removal) is **Phase 4**;
  these stubs are the honest "not yet" per the plan.

**1c. `apps/yerd-gui/src-tauri/src/daemon.rs` — `fn sigterm` (line 306)**

- Current state (un-gated, uses `libc::kill` + `libc::SIGTERM`):

  ```rust
  fn sigterm(pid: u32) {
      if let Ok(pid) = i32::try_from(pid) {
          // SAFETY: ...
          unsafe { libc::kill(pid, libc::SIGTERM); }
      }
  }
  ```

- Gate the existing body `#[cfg(unix)]`; add a `#[cfg(windows)]` no-op variant with a doc
  comment stating that graceful daemon stop on Windows arrives with the Phase 5 service
  work, and that the IPC `Shutdown` path (which `stop()` tries first) is unaffected.
  Do **not** shell out to `taskkill` here — process teardown is explicitly out of
  Phase 1 scope.
- While in this crate on Windows, expect follow-on `#[cfg]` warnings from `-D warnings`
  (e.g. `spawn_log_stdio` at `daemon.rs:323` is `target_os = "linux"` only and its
  callers must already handle other OSes — verify with `cargo clippy -p yerd-gui` on
  Windows and gate anything the compiler flags, changing no Unix behaviour).

**Verification:** `cargo check --workspace` and
`cargo clippy --workspace --all-targets -- -D warnings` pass on Windows AND Linux/macOS.

---

### Item 2 — Stub honesty: `os/windows.rs` + `active` wiring

**File: `crates/yerd-platform/src/os/mod.rs`** — current selection (lines 6-11):

```rust
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod unsupported;
```

Change to:

- `#[cfg(target_os = "windows")] mod windows;`
- Keep `unsupported` compiled for **both** Windows and other-OS builds
  (`#[cfg(not(any(target_os = "linux", target_os = "macos")))]` stays as is) because
  `windows.rs` delegates to it.
- In `pub(crate) mod active`, add a `#[cfg(target_os = "windows")]` re-export block and
  narrow the unsupported block to
  `#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]`:

  ```rust
  #[cfg(target_os = "windows")]
  pub use super::windows::{
      WindowsPaths as ActivePaths, WindowsPortBinder as ActivePortBinder,
      WindowsPortRedirector as ActivePortRedirector,
      WindowsResolverInstaller as ActiveResolverInstaller,
      WindowsSystemMetrics as ActiveSystemMetrics,
      WindowsTerminalLauncher as ActiveTerminalLauncher,
      WindowsTrustStore as ActiveTrustStore,
  };
  ```

**New file: `crates/yerd-platform/src/os/windows.rs`**

- `WindowsPaths` — the one **real** implementation this phase (Item 3).
- Every other name is a **type alias to the unsupported stub**, so the trait impls come
  for free and stay total (no half-implemented types, no duplicated stub bodies):

  ```rust
  pub use super::unsupported::{
      UnsupportedPortBinder as WindowsPortBinder,
      UnsupportedPortRedirector as WindowsPortRedirector,
      UnsupportedResolverInstaller as WindowsResolverInstaller,
      UnsupportedSystemMetrics as WindowsSystemMetrics,
      UnsupportedTerminalLauncher as WindowsTerminalLauncher,
      UnsupportedTrustStore as WindowsTrustStore,
  };
  ```

  Later phases replace one alias at a time with a real `Windows*` type **in the same
  change that adds its full trait impl** — exactly the "never half-flip" rule.
- Also home of the Windows IPC identity edge fns (Item 5): `current_user_sid()`,
  `daemon_pipe_name(&PlatformDirs)`.

**Doc touch-ups:** `unsupported.rs` module doc ("Stub implementations for unsupported
OSes (Phase 1: Windows)") and `lib.rs` module doc lines 6-8 ("Windows compiles against
the `os::unsupported` stub") need rewording; `yerd-platform.instructions.md` and
`copilot-instructions.md` say "exactly one of `linux`/`macos`/`unsupported` is active" —
update both to name `windows` (flag in the PR; instruction files are the source of
truth and must not drift).

**Tests:** add `crates/yerd-platform/tests/windows_smoke.rs` (`#![cfg(windows)]`),
mirroring `tests/unsupported.rs`: every aliased stub returns
`PlatformError::Unsupported`, and `ActivePaths::new().resolve()` succeeds (after
Item 3). Existing `tests/unsupported.rs` must now be gated so it doesn't run on
Windows if it asserts `Paths::resolve` errors — check its cfg and split accordingly.

---

### Item 3 — Paths: real `WindowsPaths` + the `for_user` fix

**3a. `WindowsPaths::resolve` (in new `os/windows.rs`)**

Do **not** use the `directories` crate here — its Windows mapping
(`%APPDATA%\Yerd\config` etc. with different casing/nesting) does not match the locked
layout. Read environment directly, mirroring how linux/macos use `ProjectDirs` as their
own convention:

| field | value | error when unset |
|---|---|---|
| `config` | `%APPDATA%\yerd` | `PlatformError::MissingHomeDir` |
| `data` | `%LOCALAPPDATA%\yerd\data` | `PlatformError::MissingHomeDir` |
| `state` | `%LOCALAPPDATA%\yerd\state` | (same) |
| `cache` | `%LOCALAPPDATA%\yerd\cache` | (same) |
| `runtime` | `std::env::temp_dir().join("yerd")` (honours `%TMP%`/`%TEMP%`, falls back internally) | infallible |

Decisions baked in: `state` stays distinct from `data` (like Linux, unlike macOS —
cheap now, avoids a migration later); `data`/`state`/`cache` are subdirectories of one
`%LOCALAPPDATA%\yerd` root so uninstall can remove a single tree + `%APPDATA%\yerd`.
Document each choice in the item docs (`///`), including the cross-platform quirk that
`runtime` is per-user because `%TEMP%` is per-user on Windows (no `/tmp` sticky-bit
trade-off; note it in the `PlatformDirs::runtime` field doc which currently only
describes Linux/macOS).

**3b. `crates/yerd-platform/src/paths.rs` — `PlatformDirs::for_user` (lines 50-88)**

Current state, the bug the plan calls out:

```rust
pub fn for_user(home: &std::path::Path, uid: u32) -> Self {
    let runtime = PathBuf::from(format!("/tmp/yerd-{uid}"));   // line 52: unconditional
    ...
    #[cfg(not(any(target_os = "linux", target_os = "macos")))] // lines 78-87
    {   // fabricates Linux-style ~/.config paths
```

Change (signature **unchanged** — recommended over the signature-rethink option because
the only non-test caller is `bin/yerd/src/uninstall.rs:82`, which lives inside the
`#[cfg(unix)] mod unix_impl` and passes a real uid; keeping the signature keeps the Unix
elevation contract byte-identical and the diff minimal):

- Move the `let runtime = ...` line **into** the `target_os = "macos"` and
  `target_os = "linux"` blocks verbatim (zero Unix behaviour change; the existing
  `for_user_layout_matches_resolve_for_current_home` drift-guard keeps it honest).
- Replace the fabricating non-Unix branch with:
  - `#[cfg(target_os = "windows")]`: derive from `home` without env reads (the
    documented contract of `for_user`): `config = home\AppData\Roaming\yerd`,
    `data/state/cache = home\AppData\Local\yerd\{data,state,cache}`,
    `runtime = home\AppData\Local\Temp\yerd`. `uid` is meaningless on Windows: consume
    it with `let _ = uid;` and a doc line saying the Windows caller (Phase 4 uninstall)
    identifies the user by `home` alone. Note in docs this is a best-effort *default
    profile shape* reconstruction (redirected/roaming profiles may differ from the live
    `resolve()` env answer — same caveat class as the documented `XDG_RUNTIME_DIR`
    gap on Linux).
  - Keep a `#[cfg(not(any(linux, macos, windows)))]` fallback (current Linux-shaped
    body + runtime line moved in) so truly-unknown OSes still compile.

**Tests (in `paths.rs`)**

- Extend the `for_user_tests` module gate to include Windows and add
  `for_user_windows_layout` asserting the exact five joins from a fake
  `C:\Users\test` home.
- Add a Windows drift-guard mirroring the Unix one: when `%APPDATA%`/`%LOCALAPPDATA%`
  sit under `%USERPROFILE%` in the default shape (guard-and-skip otherwise, like the
  Unix test skips on custom XDG vars), `for_user(USERPROFILE, 0)` config/data/state/cache
  must equal `ActivePaths::new().resolve()`.
- `windows_smoke.rs` (Item 2) asserts `resolve()` returns paths ending
  `yerd`, `yerd\data`, etc.

**Dependencies:** none new. (`directories` stays for linux/macos only.)

---

### Item 4 — Artifact enums (three crates) + forced downstream match fixes

**4a. `crates/yerd-php/src/release.rs`**

- `enum Os` (line 139): add `/// Windows.` `Windows` variant; `as_str()` (line 150) adds
  `Os::Windows => "windows"`.
- `current_os_arch()` (line 343): add `"windows" => Os::Windows,` to the OS match. Arch
  match already covers `x86_64`/`aarch64`; leave it (Windows publishes x86_64 only
  today — resolution correctly fails later with `VersionUnavailable` for
  windows-aarch64, which is accurate, not a platform error).
- Update the fn doc ("erroring on anything yerd can't install for (e.g. Windows...)")
  and the `Os::Linux` variant doc if it references "the manifest never ships" wording
  that implies two variants.
- **Tests:** add windows rows to the `LISTING` fixture
  (`php-8.5.7-2-{cli,fpm}-windows-x86_64.tar.gz`), plus:
  `resolve_from_listing(... Os::Windows, Arch::X86_64 ...)` builds the right URLs;
  `available_minors` anchors windows; and a `#[cfg(windows)]` test asserting
  `current_os_arch() == Ok((Os::Windows, Arch::X86_64))` (arch-gated) — this is the test
  that makes the Windows CI leg meaningful for this crate.

**4b. `crates/yerd-services/src/release.rs`**

- Same shape: `enum Os` (line 90) + `as_str()` (line 100) gain `Windows` / `"windows"`;
  `current_os_arch()` (line 238) gains the `"windows"` arm; doc updates.
- `platform_token`/`artifact_filename`/`artifact_url` are already generic over
  `os.as_str()`, so `redis-9.1.0-windows-x86_64.tar.gz` falls out — **exactly** the
  sibling repo's published scheme (§0.3). No filename code changes.
- **Tests:** windows rows in `LISTING`; `artifact_filename(..., Os::Windows, Arch::X86_64)`
  equals `"mysql-8.4.9-windows-x86_64.tar.gz"` (pin the contract string); resolve +
  available_versions for windows; `#[cfg(windows)]` `current_os_arch` test.

**4c. Forced fixes in `bin/yerdd` (same change as 4a — adding the variant breaks these
exhaustive matches):**

- `bin/yerdd/src/tools/node.rs:31-39` `host_platform()` — add
  `(Os::Windows, _) => return None` (yields the existing
  `ToolError::UnsupportedHost("Node.js")`). Honest stub: Node-on-Windows tooling is
  Phase 5 territory (its install path also assumes `.tar.gz` + Unix layout).
- `bin/yerdd/src/tools/bun.rs:30-38` — same, `(Os::Windows, _) => return None`.
- `bin/yerdd/src/tunnel/install.rs:325-338` `host_asset(os, arch)` — currently
  infallible `(String, bool)`. Change to `Option<(String, bool)>`
  (`Os::Windows => None` for now) and map `None` to the existing
  `CloudflaredInstallError::UnsupportedHost` in `install()` (line 354). Update the
  `host_asset` unit tests (`install.rs:545-557`) for the `Option`. Exercising the real
  `cloudflared-windows-amd64.exe` asset is Phase 5 per the plan.
- `bin/yerdd/src/services.rs:148` constructs `(Os::Linux, Arch::X86_64)` as a fallback —
  compiles fine, no change.
- `ext_install.rs` matches on `std::env::consts::OS` **strings**, not the enum — no
  change (the `yerd-dump-<minor>-windows-<arch>.dll` name comes from the manifest,
  consumed verbatim; nothing in Phase 1 constructs it).

**4d. `crates/yerd-update/src/artifact.rs`**

- `enum Platform` (line 32): add `WindowsX86_64` and `WindowsAarch64` variants with docs
  noting no self-update artifact is published yet (packaging is the still-open
  decision #5).
- `Platform::current()` (line 49): add the two
  `#[cfg(all(target_os = "windows", target_arch = ...))]` blocks and extend the final
  `#[cfg(not(any(...)))]` list so `Unsupported` no longer matches Windows.
- `select_asset` (line 213): extend the existing no-artifact arm:

  ```rust
  (p @ (Platform::MacOsX86_64
      | Platform::Unsupported
      | Platform::WindowsX86_64
      | Platform::WindowsAarch64), _) => {
      return Err(AssetError::NoArtifactForPlatform(p));
  }
  ```

- **⚠ Deliberate deviation from WINDOWS_PLAN.md (flagged for confirmation):** the plan
  says "add `is_windows_*` matchers" in Phase 1. I recommend **deferring the matcher
  fns to Phase 6** because (a) the artifact names they'd match don't exist — NSIS-vs-MSI
  and naming are explicitly open until Phase 6 planning; (b) wiring them into
  `select_asset` requires a new `ArtifactKind` variant, which cascades into the
  **`yerd-ipc` wire enum `StagedArtifact`** (`bin/yerdd/src/self_update.rs:434` maps
  `ArtifactKind → StagedArtifact` exhaustively, and `bin/yerd/src/apply.rs:242` matches
  `StagedArtifact` exhaustively into apply flows) — an IPC-contract change with
  wire-stability tests, squarely Phase 6 scope; (c) unwired private matcher fns fail
  `-D warnings` dead-code on every OS. The `Platform` variants land now (they're what
  Phases 2-6 key off); matchers land with the artifact they match. If the letter of the
  plan is preferred, the alternative is: add `ArtifactKind::Nsis` + `StagedArtifact`
  variant + apply-path stubs now — call it out before implementing.
- **Tests:** `current_platform_is_known_on_dev_hosts` (line 778) gets windows added to
  its cfg list asserting `p != Platform::Unsupported`; a new
  `windows_has_no_selfupdate_artifact_yet` test pins
  `select_asset(&r, Platform::WindowsX86_64, PkgFormat::Deb) == Err(NoArtifactForPlatform(...))`.
- Callers `bin/yerdd/src/ipc_server.rs:4863,4926,4969,5031`, `self_update.rs:391`,
  `main.rs:64` compile unchanged (no exhaustive `Platform` matches outside the crate —
  verified by grep).

---

### Item 5 — IPC: deterministic SID-keyed pipe + explicit DACL + client wiring

**Current state.**

- Daemon `bin/yerdd/src/startup.rs:821-831` (inside `build_ipc_listener`):

  ```rust
  #[cfg(windows)]
  let name = {
      use interprocess::local_socket::{GenericNamespaced, ToNsName};
      let pipe = format!("yerd-{}", std::process::id());
      ...
  };
  ```

- CLI `bin/yerd/src/transport.rs:53-59`: `#[cfg(not(unix))] exchange` returns
  `ClientError::DaemonUnreachable("...pipe name is non-deterministic")`.
- GUI `apps/yerd-gui/src-tauri/src/ipc.rs:79-84`: identical stub.
- `crates/yerd-ipc` itself is **not touched**: its transport is generic over
  `AsyncRead/AsyncWrite` and its instructions forbid owning a socket/pipe. The plan
  doc's phrase "wire the client exchange in `yerd-ipc`" actually lands in
  `bin/yerd/src/transport.rs` and `src-tauri/src/ipc.rs` (the two existing exchange
  sites) plus shared name-derivation in `yerd-platform`. Flagging this reading
  explicitly.

**5a. Pipe-name + DACL derivation — `crates/yerd-platform/src/pure/win_pipe.rs` (new)**

Pure, compiled on **all** OSes (so Linux/macOS CI table-tests it too, per the
"decisions in pure helpers" rule). Three functions:

- `pub fn pipe_name(sid: &str, runtime: &Path) -> String` →
  `format!("yerd-{sid}-{h}")` where `h` = first 16 hex chars of
  `sha2::Sha256(runtime.as_os_str() bytes, lossy-normalised)`. Rationale (document in
  module doc):
  - **SID component** — the locked decision: global, per-user-unique, and what the
    Phase 5 session-0 service will key the DACL on. (Named-pipe names are inherently
    global — `\\.\pipe\` has no per-session namespace — so cross-session reachability
    is about the DACL, not a `Global\` prefix; record this in the doc.)
  - **runtime-dir hash component** — production determinism is preserved (daemon and
    clients both resolve the same `%TEMP%\yerd` via `ActivePaths`), while
    tempdir-rooted integration tests get collision-free names, letting the three
    lifecycle tests run in parallel and coexist with a real installed daemon —
    the same isolation property the Unix tempdir socket path provides. The Phase 5
    service already has to reconstruct the target user's `PlatformDirs` for
    config/data, so this adds no new Phase 5 burden.
- `pub fn pipe_sddl(sid: &str) -> String` →
  `format!("D:P(A;;GA;;;SY)(A;;GA;;;{sid})")` — protected DACL (no inheritance),
  full access for SYSTEM (so the Phase 5 service account can own/serve it) and for the
  named user; everyone else denied by absence. This is the explicit DACL the plan
  requires now, not deferred.
- `pub fn parse_whoami_sid(stdout: &str) -> Option<String>` — parse
  `whoami /user /fo csv /nh` output (`"machine\user","S-1-5-21-…"`): take the last
  CSV field, strip quotes/whitespace, validate `starts_with("S-1-")` and charset
  `[0-9-]` after the prefix (also guards the SDDL/pipe-name injection surface: reject
  anything else).

Register in `pure/mod.rs`. Table tests: name shape/stability (golden string for a fixed
sid+path), sid parse for real-shaped CSV, localized junk, empty, quote-less variants,
rejection cases; sddl golden string.

**5b. SID lookup edge — `os/windows.rs`**

- `pub fn current_user_sid() -> Result<String, PlatformError>`: run
  `%SystemRoot%\System32\whoami.exe /user /fo csv /nh` (absolute path — do not trust
  `PATH`), parse via `parse_whoami_sid`, cache in a `std::sync::OnceLock<String>` (one
  spawn per process; the CLI calls it once per invocation). No `unsafe`, no new crates.
- Error: add a typed variant to `PlatformError` (it is `#[non_exhaustive]`):
  `#[error("could not determine the current user SID: {detail}")] SidLookup { detail: String }`
  — update `error.rs`'s `construct_every_variant` tripwire test.
- `pub fn daemon_pipe_name(dirs: &PlatformDirs) -> Result<String, PlatformError>` =
  `pipe_name(&current_user_sid()?, &dirs.runtime)` — the single shared derivation.
- Export from `lib.rs` under `#[cfg(target_os = "windows")]`
  (`pub use os::windows::{current_user_sid, daemon_pipe_name};`).
- **Escalation option (decision, only if shell-out is rejected):** the no-subprocess
  route is `OpenProcessToken`/`GetTokenInformation(TokenUser)` via `windows-sys`, which
  is `unsafe` FFI — it would need a per-crate `unsafe_code = "allow"` (precedent:
  `apps/yerd-gui/src-tauri` lifts the forbid for documented FFI edges) plus `// SAFETY:`
  comments. Recommend the `whoami` route; it keeps `#![forbid(unsafe_code)]` intact in
  `yerd-platform`.

**5c. Daemon listener — `bin/yerdd/src/startup.rs::build_ipc_listener` (line 805)**

Replace the `#[cfg(windows)]` name block:

```rust
#[cfg(windows)]
let (name, sddl) = {
    use interprocess::local_socket::{GenericNamespaced, ToNsName};
    let sid = yerd_platform::current_user_sid().map_err(...)?;
    let pipe = yerd_platform::pure::win_pipe::pipe_name(&sid, &dirs.runtime);
    let sddl = yerd_platform::pure::win_pipe::pipe_sddl(&sid);
    (pipe.to_ns_name::<GenericNamespaced>().map_err(...)?, sddl)
};
```

and extend listener construction: on Windows build the options as

```rust
use interprocess::os::windows::local_socket::ListenerOptionsExt;
use interprocess::os::windows::security_descriptor::SecurityDescriptor;
let sd = SecurityDescriptor::deserialize(
    &widestring::U16CString::from_str(&sddl).map_err(...)?,
).map_err(...)?;
let listener = ListenerOptions::new().name(name).security_descriptor(sd).create_tokio()...;
```

(cfg-split so the Unix path — `remove_file` + `GenericFilePath` + post-bind
`restrict_to_owner` — is byte-identical.) Error mapping stays `DaemonError::Io` with a
descriptive pseudo-path (existing pattern: `PathBuf::from(&pipe)`); `DaemonError`
already wraps `PlatformError` for the SID failure (verify a `From` exists — `bring_up`
already propagates `ActivePaths::resolve()?`, so it does).

**5d. CLI client — `bin/yerd/src/transport.rs`**

- Keep the `#[cfg(unix)]` `exchange`/`exchange_at` untouched.
- Replace the `#[cfg(not(unix))] exchange` stub with a real Windows implementation:
  resolve `ActivePaths::new().resolve()?` → `daemon_pipe_name(&dirs)?` →
  `exchange_at_name(&name, req)`.
- New `#[cfg(windows)] pub async fn exchange_at_name(name: &str, req: &Request)`:
  mirror of `exchange_at` using
  `GenericNamespaced`/`ToNsName` + `IpcStream::connect`, then the identical
  `write_message`/`FrameDecoder`/`read_message` body (factor the post-connect exchange
  into a small private helper shared by both cfg arms to avoid divergence).
  Map `PlatformError::SidLookup`/connect failures to
  `ClientError::DaemonUnreachable` exactly as the Unix arm does.
- Update the module doc (lines 1-6) to describe both derivations.

**5e. GUI bridge — `apps/yerd-gui/src-tauri/src/ipc.rs`**

- Same treatment as 5d (it is documented as "a near-verbatim mirror of
  `bin/yerd/src/transport.rs`"): real `#[cfg(not(unix))] exchange` via
  `daemon_pipe_name`, errors as `GuiError::unreachable`. `exchange_timeout` is
  transport-agnostic and needs no change.
- `daemon.rs:457` and `logging.rs:381` display `runtime.join("yerd.sock")` in
  diagnostics — on Windows show the pipe name instead (small cfg’d helper,
  display-only).

**Dependencies (all pinned per repo rules):**

- Root `Cargo.toml` `[workspace.dependencies]`: add
  `widestring = "1"` (already in `Cargo.lock` at 1.x via `interprocess` — reuse, don't
  bump).
- `bin/yerdd/Cargo.toml`, `bin/yerd/Cargo.toml`, `apps/yerd-gui/src-tauri/Cargo.toml`:
  `[target.'cfg(windows)'.dependencies] widestring.workspace = true`.
- `interprocess` is already a workspace dep with `tokio` feature — its Windows
  `ListenerOptionsExt`/`SecurityDescriptor` need **no extra feature** (verified in
  2.4.2 source: the `os::windows` module is unconditional on Windows).
- No `windows`/`windows-sys` direct dependency needed (only as the flagged 5b fallback).

**Tests:** pure tests in 5a; the end-to-end proof is Item 6. Optionally a
`#[cfg(windows)]` unit test in `startup.rs` asserting `build_ipc_listener` succeeds
against a tempdir `PlatformDirs` and a second listener on the same dirs fails
(name uniqueness + AlreadyExists behaviour).

---

### Item 6 — Port `bin/yerdd/tests/lifecycle.rs` to Windows

Current state: entire body inside `#[cfg(unix)] mod tests` (line 12); connections built
with `GenericFilePath` from `dirs.runtime.join("yerd.sock")` (lines 87, 138, 195, 221,
231); `boot_ping_shutdown_round_trip` asserts `ipc_sock.exists()` (line 222).

Changes:

- Drop the `#[cfg(unix)]` on the module; all three tests
  (`park_round_trip_persists`, `set_secure_round_trip_persists`,
  `boot_ping_shutdown_round_trip`) and `drive_subsystems` are already
  platform-neutral apart from connection setup.
- Add one cfg-split helper and route everything through it:

  ```rust
  async fn connect_ipc(dirs: &yerd_platform::PlatformDirs) -> IpcStream {
      #[cfg(unix)]  { /* GenericFilePath from dirs.runtime.join("yerd.sock") */ }
      #[cfg(windows)] { /* GenericNamespaced from yerd_platform::daemon_pipe_name(dirs) */ }
  }
  ```

  `round_trip` takes `&PlatformDirs` instead of a socket path.
- Replace the `ipc_sock.exists()` pre-assertion with cfg: on Unix keep it; on Windows
  the bind-success of `bring_up_with_dirs` plus the subsequent connect is the
  assertion (named pipes aren't filesystem-visible the same way).
- Watch item: `park_round_trip_persists` compares the config file against
  `std::fs::canonicalize(&sites_root)` (line 107) — on Windows `canonicalize` yields a
  `\\?\C:\...` extended-length path. Whether the assertion holds depends on the Park
  handler canonicalizing identically (it should, same API). If it fails, normalise both
  sides via a small `dunce`-free strip of the `\\?\` prefix **in the test only** — do
  not change daemon behaviour in this phase; record whatever is found.
- Parallel-safety: guaranteed by the runtime-dir hash in the pipe name (Item 5a) —
  each test's tempdir gives a distinct pipe.
- Keep the file's existing clippy allow-block; no new test files needed.

This is the phase's Definition-of-Done anchor: on the Windows CI leg these three tests
must **execute** (verify with `cargo test -p yerdd --test lifecycle -- --list` locally,
and see the CI guard in Item 8).

---

### Item 7 — Platform identity in the GUI

**`apps/yerd-gui/src/composables/usePlatform.ts`** — current interface (lines 28-43)
exposes `isMac`, `isLinux`, `supportsPathInstall` only.

- Add `isWindows: ComputedRef<boolean>` (`platform.value === "windows"`) to
  `PlatformInfo` and `usePlatform()`.
- Leave `supportsPathInstall` as macos/linux — Windows PATH install is Phase 5; its
  doc comment already says so.
- No backend change (see §0.4). No change to `TitleBar.vue` /
  `EnvironmentCard.vue` in this phase; they consume the same `hostPlatform()` and
  already branch on the string.
- **Test:** new `apps/yerd-gui/src/composables/usePlatform.test.ts` (vitest, mocking
  `@/ipc/client`'s `hostPlatform` — same pattern as `useDaemonStart.test.ts`):
  `windows` → `isWindows` true, `isMac`/`isLinux`/`supportsPathInstall` false; retry-
  after-failure path keeps working.

---

### Item 8 — CI: `windows-latest` leg

**`.github/workflows/ci.yml`** — the `rust` job matrix (line 43):

```yaml
os: [ubuntu-22.04, ubuntu-22.04-arm, macos-14]
```

- Add `windows-latest`. Existing per-step gating already does the right thing:
  GUI deps install, `cargo fmt`, and the pacman/rpm/mutual-exclusion steps are all
  `if: runner.os == 'Linux'`; `Clippy` and `Test` run everywhere. Update the matrix
  comment to say what the Windows leg gates.
- Add a **non-vacuous-green guard** right after `Test`, Windows-only:

  ```yaml
  - name: Assert lifecycle tests ran on Windows
    if: runner.os == 'Windows'
    shell: bash
    run: |
      n=$(cargo test -p yerdd --test lifecycle -- --list 2>/dev/null | grep -c ': test$' || true)
      if [ "$n" -lt 3 ]; then
        echo "::error::lifecycle tests are cfg'd out on Windows ($n listed)"; exit 1
      fi
  ```

  This encodes the plan's "verify it's not cfg'd out" DoD clause permanently.
- Expect and budget for: `cargo test --workspace` compiling the Tauri GUI crate on the
  Windows runner (WebView2 SDK comes via the `webview2-com` build — works on hosted
  runners; no extra deps step needed), and Defender-related slowness — `Swatinem/rust-cache`
  already in place. If the leg is too slow, an acceptable Phase 1 fallback is
  `cargo clippy --workspace --all-targets -- -D warnings` + `cargo test --workspace --exclude yerd-gui`
  plus `cargo check -p yerd-gui` — but try the full gate first.
- Do **not** touch `build.yml` (release legs are Phase 6).

---

## 9. Ordering & compile checkpoints

```
Item 1  (compile floor)                      ── prerequisite for everything
Item 2  (os/windows.rs + active wiring)      ── needs 1a; lands WITH Item 3a
Item 3  (WindowsPaths + for_user)            ── 3a must land in the same change as 2
                                                (active must alias a real Paths impl)
Item 4  (artifact enums + yerdd match fixes) ── independent of 2/3/5; 4a+4c atomic,
                                                4b atomic, 4d atomic
Item 5  (IPC)                                ── needs 2+3 (runtime dir + exported helpers)
Item 6  (lifecycle port)                     ── needs 5
Item 7  (usePlatform)                        ── independent
Item 8  (CI leg)                             ── last (leg must be green when it lands);
                                                run the full gate locally on Windows
                                                after each prior item regardless
```

After every item: `cargo fmt --all --check`,
`cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` on
Windows **and** Linux/macOS (macOS/Linux byte-identical behaviour is a hard DoD line).

## 10. `unsafe` / escalation flags (none required, two contingencies)

1. **Not needed for the listener:** `interprocess` 2.4.2's safe
   `SecurityDescriptor::deserialize` + `ListenerOptionsExt::security_descriptor`
   cover the SDDL DACL (verified in source, §0.2). If implementation uncovers a gap
   (e.g. `create_tokio` ignoring the SD for the *first* pipe instance — test this
   explicitly with a second-user or deny-SDDL probe), the fallback is raw
   `CreateNamedPipeW` + `SECURITY_ATTRIBUTES` via `windows-sys`: that is `unsafe` FFI
   and needs a scoped forbid-lift (per-crate `unsafe_code = "allow"` in `bin/yerdd`
   only, GUI-crate precedent) — **stop and surface before doing this**.
2. **Not needed for SID lookup:** `whoami.exe` shell-out (System32 absolute path) keeps
   `yerd-platform` at `#![forbid(unsafe_code)]`. The `GetTokenInformation` alternative
   is `unsafe` — same escalation protocol if chosen.

## 11. Deviations from WINDOWS_PLAN.md (surface, per CLAUDE.md)

1. **`is_windows_*` matchers deferred to Phase 6** (Item 4d): artifact names don't
   exist yet (open decision #5) and wiring them forces an additive `yerd-ipc`
   `StagedArtifact` wire change + apply-path stubs that are Phase 6 scope.
   `Platform::Windows*` variants DO land now.
2. **"Wire the client exchange in `yerd-ipc`"** is implemented in
   `bin/yerd/src/transport.rs` + `src-tauri/src/ipc.rs` + `yerd-platform` helpers —
   `yerd-ipc` stays codec-pure per its own instruction file (no socket/pipe ownership).
3. **"daemon `hostPlatform()` returns `windows`"** requires no code change — it is a
   host-side Tauri command already returning `std::env::consts::OS` (§0.4); the only
   work is the frontend gate (Item 7).
4. **Pipe name includes a runtime-dir hash** in addition to the locked SID key —
   pure win for test isolation with zero production or Phase 5 cost (rationale in 5a).

## 12. Definition of done (restating the plan, made checkable)

- [ ] `cargo fmt/clippy -D warnings/test --workspace` green on ubuntu-22.04,
      ubuntu-22.04-arm, macos-14, **windows-latest**.
- [ ] CI's "Assert lifecycle tests ran on Windows" guard passes (≥3 tests listed).
- [ ] On a Windows machine: `cargo run -p yerdd -- serve` starts; `cargo run -p yerd -- status`
      round-trips over the named pipe; `%APPDATA%\yerd` and `%LOCALAPPDATA%\yerd\*`
      appear; pipe is `\\.\pipe\yerd-<SID>-<hash>` with the explicit DACL
      (inspect with `accesschk \pipe\yerd-*` or PowerShell `GetAccessControl`).
- [ ] macOS/Linux behaviour byte-identical: no Unix diff outside cfg-moves; the
      `for_user` drift-guard and all existing tests untouched-and-green.
- [ ] Instruction-file updates (`yerd-platform.instructions.md`,
      `copilot-instructions.md` cross-platform section) included in the same change
      that adds `os/windows.rs`.
