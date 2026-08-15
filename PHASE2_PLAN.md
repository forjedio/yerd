# Phase 2 Implementation Plan — Run things: supervision, extraction, PHP, services

Working doc (not committed). Companion to `WINDOWS_PLAN.md` Phase 2 and successor to
`PHASE1_PLAN.md` (Phase 1 is committed as `ed9add2`; the `windows-latest` CI leg and
`os/windows.rs` exist). Everything below was verified against the actual code, the
sibling repos at `C:\code\yerd-php` / `C:\code\yerd-services`, and the **live published
manifests** on 2026-08-02, including `cargo check` runs on this Windows machine.
§0.1 and Item 1 were **re-verified and revised on 2026-08-03** after the producer
split Windows PHP into its own manifests (see §0.1) — the earlier "php.json hotfix"
framing is gone.

---

## 0. Ground truth (verified, not assumed)

### 0.1 Windows PHP lives in SEPARATE per-channel manifests (`php-windows*.json`) — the earlier `php.json` breakage is fixed upstream

The producer has **split Windows out of `php.json` entirely** (yerd-php
`scripts/config.sh:127-152` `WINDOWS_CHANNELS`/`WINDOWS_MANIFEST_NAME`,
`generate-manifest.php:71-117` `--windows` flag, `refresh-manifests.sh`,
`publish.sh`). Re-verified against the live release on **2026-08-03**
(`generated_at: 2026-08-03T00:57:24Z`):

- `php.json` / `php-legacy.json` (unix) are **pure `cli`/`fpm` again — zero windows
  rows** (grepped the live body: 0 matches). The existing Unix parser is unbroken;
  there is **no production bug and no hotfix**.
- Windows is published as `php-windows.json` (stable) and `php-windows-legacy.json`
  (legacy), each with a detached `.minisig` — all four assets live (HTTP 200).
- Same top-level shape as `php.json` (`{"schema": 1, "generated_at": "…",
  "builds": [...]}`), but each build entry carries a single **`bundle`** object,
  no `cli`/`fpm` keys:

```json
{ "php": "8.4.24", "minor": "8.4", "os": "windows", "arch": "x86_64", "revision": 1,
  "bundle": { "file": "php-8.4.24-1-bundle-windows-x86_64.tar.gz", "sha256": "14b1…", "size": 49030790 } }
```

  Live rows today: 8.2.33-1, 8.3.33-1, 8.4.24-1, 8.5.9-1 (`x86_64` only).
- Signing is **identical** to the unix manifests: the repo's dedicated key
  (embedded in yerdd as `PHP_LISTING_PUBLIC_KEY`), prehashed `minisign -S -H`
  (`sign-manifest.sh`), so the daemon's existing
  `verify_minisign(bytes, sig, allow_legacy=false)` path needs **no change beyond
  the file names**.

Consequence: **Item 1 is no longer a standalone cross-platform hotfix.** It is
plain Phase-2 feature work — make the Windows daemon fetch/verify/parse the
dedicated windows manifests — plus correcting the Phase-1 test fixtures that pin
the wrong (cli/fpm windows-row) shape (`release.rs:451-453,:503-534`). The
previously planned `Option<FileEntry>`-everywhere + `ArtifactDownloads` union in
the shared `php.json` parser is **not needed**: the shapes never mix in one file.

### 0.2 The Windows PHP artifact contract (from `C:\code\yerd-php`, confirmed live)

- Listed in `php-windows.json` / `php-windows-legacy.json` (§0.1), never in
  `php.json`. One artifact per version:
  `php-<ver>-<rev>-bundle-windows-x86_64.tar.gz` — a
  **`.tar.gz` directory bundle** (`repackage-windows.sh`, `package-artifacts.sh:42-47`),
  NOT a zip and NOT single-file. It unpacks to:
  `php.exe`, `php-cgi.exe`, `php-win.exe`, `php8.dll`, `php.ini` (generated,
  `extension_dir = "ext"`), `cacert.pem`, support DLLs (`libpq.dll`, …), `ext/*.dll`.
- **There is no `php-fpm.exe` — ever.** FPM is a Unix-only SAPI; the FastCGI server
  binary on Windows is `php-cgi.exe` (single-threaded NTS build). The current
  `FPM_BINARY_PATH` non-unix arm `&["php-fpm.exe"]`
  (`crates/yerd-php/src/version.rs:17-18`) can never match anything published.
- **Install-time coordination point the sibling README demands:** `php.ini` ships
  `curl.cainfo` / `openssl.cafile` **commented out**; PHP resolves those relative to
  CWD, so the daemon must append the **absolute** extracted `cacert.pem` path at
  install time or outbound HTTPS from PHP fails verification.

### 0.3 The Windows services contract (from `C:\code\yerd-services` + live `services.json`)

- Uniform `.tar.gz`, `<svc>-<ver>-windows-x86_64.tar.gz`, `bin/` layout with runtime
  DLLs beside the exes. The consumer's `artifact_filename` already produces this
  (`crates/yerd-services/src/release.rs:151-159`); no listing-shape change needed
  (`services.json` parses fine — verified against the live body).
- Published for `windows-x86_64` **today**: meilisearch 1.49.0, mysql 8.4.9 / 9.7.1,
  postgres 17.10 / 17.10-full / 18.4 / 18.4-full, versitygw 1.7.0.
  **Not yet published:** redis, mariadb (build scripts exist; listing rows absent, so
  `yerd service install` correctly reports `VersionUnavailable` until the sibling
  publishes — no code needed to "handle" the absence).
- Binary names inside the Windows tarballs (from `build-service.sh`):
  - redis slot → **`bin/redis-server.exe` / `bin/redis-cli.exe`** (native MSVC Redis
    port — NOT `valkey-server`; a real name divergence, not just an `.exe` suffix).
  - mysql → `bin/mysqld.exe`, `mysql.exe`, `mysqldump.exe` (+ `share/`, `lib/plugin`).
  - mariadb → `bin/mariadbd.exe`, `mariadb.exe`, `mariadb-dump.exe`, and an init tool
    whose name **varies per bundle** (`mariadb-install-db.exe` or
    `mysql_install_db.exe` — the producer probes; the consumer must too).
  - postgres → `bin/postgres.exe`, `initdb.exe`, **`pg_ctl.exe`** (guaranteed by
    `require_files`), `psql.exe`, `pg_dump.exe`, … (+ `share/`, `lib/` verbatim).
  - meilisearch → `bin/meilisearch.exe`; versitygw → `bin/versitygw.exe`.
  The consumer's names are the Unix ones with no `.exe`
  (`crates/yerd-services/src/service.rs:401,453-457,528-532,603-610,695-696` and
  `SqlEngine::client_binary`/`dump_binary` at `service.rs:96-112`).

### 0.4 The `.zip` question — answered: **no Phase-2 artifact is a zip**

PHP bundle: tar.gz. All services: tar.gz. `yerd-dump`/`pcov`: bare `.dll` files
(manifest-driven, no archive — live `manifest.json` of `yerd-php-ext` already carries
`windows` rows, verified). cloudflared Windows is a bare `.exe` (Phase 5). Self-update
archives are Phase 6. The `zip` crate is already a workspace dep (root
`Cargo.toml:27`, pinned at 2.x with a comment; used by `bin/yerdd/src/tools/bun.rs`
with `#[cfg(unix)]`-gated mode bits, i.e. already Windows-safe). **Deviation from
WINDOWS_PLAN.md, flagged in §9: no new `.zip` dispatch is built in Phase 2** — there is
nothing for it to dispatch on. The chmod-skip half of the extraction item is likewise
already done: `php_install::make_executable` has a `#[cfg(not(unix))]` no-op
(`php_install.rs:483-487`), `service_install::extract_all` uses the `tar` crate whose
mode-bit application is Unix-only internally, and the `tar -xpf` shell-out
(`bin/yerd/src/apply.rs:433`) sits inside the **macOS `.app`** update path that Windows
never reaches (Windows self-update is Phase 6).

### 0.5 Supervision today: what actually leaks on Windows

- All `process_group(0)` call sites are **already `#[cfg(unix)]`-gated** (Phase 1's
  compile floor): `crates/yerd-php/src/manager.rs:779-783`,
  `crates/yerd-services/src/manager.rs:925-933` (`set_own_process_group`),
  `crates/yerd-tunnel/src/manager.rs:491-495`, plus out-of-phase sites
  (`yerd-service-ctl/src/lib.rs:180` is `cfg(linux)`, `bin/yerdd/src/create_site/mod.rs:322`,
  `bin/yerd/src/apply.rs`, GUI `commands.rs`). The WINDOWS_PLAN scope line "cfg-gate the
  Unix `process_group(0)` calls" is **already satisfied**; the real Phase-2 work is
  giving Windows an equivalent, not the gating.
- The Windows kill path (`crates/yerd-supervise/src/real.rs:153-158`): ignores
  `KillSignal`/`StopProtocol` and calls `self.inner.kill()` — `TerminateProcess` on the
  **direct child only** (marked `TODO(Phase 2)`). `kill_process_group` is a no-op off
  Unix (`real.rs:58-59`), so `InitGroupReaper`
  (`yerd-services/src/manager.rs:935-955`) silently does nothing on Windows.
  `kill_on_drop(true)` (`real.rs:70`) also only covers the direct child. Leaks on
  Windows today: FPM/php-cgi workers, mysqld/mariadbd spawned by an init script,
  anything a supervised child forks.
- `yerd-supervise` **has `#![forbid(unsafe_code)]`** (`src/lib.rs:20`) on top of the
  workspace `unsafe_code = "forbid"` (root `Cargo.toml:70-71`) via `[lints] workspace = true`.
- Both consumers instantiate `TokioProcessSpawner` (`bin/yerdd/src/backend_resolver.rs:13`,
  `bin/yerdd/src/services.rs:36`, `startup.rs:219-221`), so a fix inside
  `TokioProcessSpawner::spawn` / `TokioChild` covers PHP, services, tunnel, and the
  init tools in one place.

### 0.6 Two latent blockers found that WINDOWS_PLAN does not list

1. **`WindowsPortBinder` is still the Unsupported stub**
   (`crates/yerd-platform/src/os/windows.rs:17`). The `#[cfg(windows)]` branch of
   `AllocatedListen::plan` (`crates/yerd-php/src/listen.rs:68-79`) calls
   `binder.bind(0)` — which returns `PlatformError::Unsupported` on Windows today, so
   **no FPM pool can even plan a port**. WINDOWS_PLAN puts PortBinder in Phase 3, but
   the trait impl is a hard Phase-2 dependency. Deliberate pull-forward (§9): implement
   the full `PortBinder` trait (plain `TcpListener` binds; `bind_pair` is the same
   generic retry logic as Linux minus setcap) in Phase 2; Phase 3 then only validates
   80/443 conflicts and adds the doctor check.
2. **`php-cgi.exe` restart-storm trap**: `php-cgi` exits after `PHP_FCGI_MAX_REQUESTS`
   (default 500) requests. The supervisor counts that as a crash
   (`max_restart_attempts: 3`, `SupervisorPolicy::fpm()`), so an unconfigured pool
   would go `PermanentFailure` after ~1500 requests. The spawn env must set
   `PHP_FCGI_MAX_REQUESTS=0` (Item 5).

### 0.7 Misc verified facts

- `yerd-mail` is an **in-process tokio SMTP sink** (`crates/yerd-mail/src/lib.rs`) —
  no child process exists to supervise. WINDOWS_PLAN's "`yerd-mail` likewise
  [supervised under Job Objects]" is moot; it works the moment the daemon runs
  (deviation, §9).
- FastCGI TCP branches already exist and are OS-clean: probe
  (`crates/yerd-php/src/io/fastcgi_probe.rs:37-41`) and proxy
  (`crates/yerd-proxy/src/forward/fcgi.rs:285-290`).
- The "~14 pending tests": `bin/yerdd/src/ipc_server.rs` has 12 `#[cfg(unix)]`
  install/discovery/poll tests plus 2 gated helpers, with an explicit comment at
  `ipc_server.rs:3692-3696` deferring them to this phase (`fake_install` lays down
  `bin/php` + `sbin/php-fpm`). Also Unix-gated for layout reasons:
  `yerd-php/src/version.rs` discover test, `yerd-php/src/listen.rs` plan test,
  `yerd-services/src/config_render.rs` goldens (`:227,297,311` — path-separator only).
- Postgres def: `stop_protocol = MasterInterrupt` (`service.rs:618-620`) → SIGINT to
  the master on Unix (`real.rs:149`); datadir is major-pinned; `pg_ctl.exe` ships in
  the tarball. `initdb` args (`service.rs:630-640`) are portable. Note: `postgres.exe`
  **refuses to run from an elevated (admin) token** — worth a doctor line eventually,
  a documented caveat now.
- Doctor is pure over the IPC `StatusReport`
  (`crates/yerd-doctor/src/lib.rs:49 diagnose`), so any *new* doctor warning requires
  an **additive IPC field** — an IPC-contract touch that must be called out (§8).
- Live `yerd-php-ext` `manifest.json` already lists `windows` DLL rows;
  `ext_install.rs` matching is OS-string generic (`ext_install.rs:136-149`), but the
  on-disk names hardcode `.so` (`ext_install.rs:56-63`, tmp name `:211`) and
  `PhpManager` joins `"yerd-dump.so"` (`yerd-php/src/manager.rs:310`).
- `cargo check -p yerd-supervise -p yerd-php -p yerd-services -p yerd-tunnel` is green
  on this machine today (run 2026-08-02).

---

## ESCALATION — Job Objects vs `#![forbid(unsafe_code)]` (decide before Item 3)

**The question:** the workspace forbids `unsafe` (root `Cargo.toml:70-71`) and
`yerd-supervise` doubles down with `#![forbid(unsafe_code)]` (`lib.rs:20`). Raw Job
Objects FFI (`CreateJobObjectW` / `SetInformationJobObject` /
`AssignProcessToJobObject` / `TerminateJobObject` via `windows`/`windows-sys`) is
`unsafe`. Is there a safe path?

**Answer: yes — a safe wrapper crate exists and its dependency tree is already in our
lockfile.** Verified from the published source of **`win32job` 2.0.3**
("A safe API for Windows' job objects", MIT/Apache-2.0, ~6 source files):

- Public API is entirely safe: `Job::create()`, `ExtendedLimitInfo::new()` +
  `.limit_kill_on_job_close()`, `Job::create_with_limit_info(..)`,
  `Job::assign_process(handle: isize)` (all `unsafe` is internal to the crate).
- `impl Drop for Job` closes the handle → with `KILL_ON_JOB_CLOSE` set, **dropping the
  `Job` terminates the whole tree**. The crate exposes no `TerminateJobObject`
  wrapper, but drop-to-kill is semantically identical for our use (and is exactly the
  kill-on-close guarantee: if `yerdd` itself dies, the OS closes the handle and the
  tree dies with it).
- Dependencies: `thiserror 1.0` and `windows = "0.61"` — **both already resolve in
  `Cargo.lock`** (`windows 0.61.3` via Tauri; `thiserror 1.0.69` alongside 2.x). Net
  new crates in the lock: `win32job` itself, nothing else. Edition 2021, no
  `rust-version` floor conflict with the workspace's 1.77.
- The process handle comes from `tokio::process::Child::raw_handle()` — a safe method.

**Option A — `win32job` dep (recommended).**
`[workspace.dependencies] win32job = "2"` + `[target.'cfg(windows)'.dependencies]`
in `yerd-supervise` (mirroring its existing `cfg(unix)` `nix` dep).
*Pros:* zero `unsafe` in our code; both `forbid`s stay intact; one integration point
(`TokioProcessSpawner::spawn`) covers PHP + services + tunnel + init tools; kill-on-
close covers daemon crash, not just orderly shutdown; nested-job semantics fine on
Win8+/GitHub runners (which themselves run inside a job).
*Cons:* a new third-party dep on the supervision path (small, stable, auditable in an
afternoon); a residual **spawn→assign race** — a child could `CreateProcess` a
grandchild in the microseconds before `AssignProcessToJobObject` lands, and that
grandchild would escape the job. Closing that race needs `CREATE_SUSPENDED` + resume
(raw `unsafe` FFI, not exposed by std/tokio). None of our workloads (php-cgi, mysqld,
postgres, cloudflared, init tools) spawn children before finishing their own exe/DLL
load, so this is accepted residual risk, documented in the module doc.

**Option B — scoped forbid-lift + raw `windows-sys` FFI.**
Per-crate `unsafe_code = "allow"` in `yerd-supervise` (the GUI `src-tauri` crate is
the precedent: `apps/yerd-gui/src-tauri/Cargo.toml:105-110`, `// SAFETY:` comments on
each block), remove `lib.rs:20`.
*Pros:* no third-party wrapper; access to `TerminateJobObject` proper and (if ever
wanted) the suspended-spawn fix for the assign race.
*Cons:* the blast radius is the whole crate — `yerd-supervise` permanently loses the
`forbid`, and it is the crate that spawns and kills processes for the entire product;
every future edit there can now introduce `unsafe`. Contradicts the "No `unsafe`"
hard rule for materially less benefit than the GUI's unavoidable FFI edges.

**Option C — `taskkill /PID <pid> /T /F` subprocess (no unsafe, no new dep).**
Shell out at kill time.
*Pros:* trivially safe; nothing new in the dep graph.
*Cons (why it fails the phase DoD):* `/T` walks the parent-PID tree **at kill time** —
if an intermediate parent already exited (exactly what `mariadb-install-db` style
bootstrap scripts do), its children are orphaned out of the tree and are missed;
parent-PID reuse can mis-kill; and above all there is **no kill-on-close guarantee**:
a crashed/killed `yerdd` reaps nothing, whereas the DoD requires "shutdown leaves zero
orphaned processes" including the crash path. Acceptable only as a stopgap if Option A
is vetoed, and would need a doctor/README caveat.

**Recommendation: Option A.** It is the only option that satisfies both the no-unsafe
hard rule and the orphan-free DoD. It needs a human "yes" only because it adds a
pinned third-party dependency to the supervision path (repo rule: pin + understand
deps), not because any lint is lifted. If vetoed: Option C as explicit MVP-with-caveat,
never Option B without a separate decision.

**No other Phase-2 item needs `unsafe`** — bundle extraction, `pg_ctl` shell-out,
`php-cgi` spawning, `TcpListener` binds and the FastCGI probe are all std/tokio safe
APIs (verified against each file touched below).

---

## Implementation checklist

Ordered; each item ends at a compiling workspace on all three OSes
(`cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings &&
cargo test --workspace`). Unix behaviour must remain byte-identical except where a
golden is deliberately regenerated (called out inline).

### Item 1 — Windows manifest consumption: `php-windows*.json` + bundle resolve (`crates/yerd-php/src/release.rs`, callers)

Feature work for §0.1 (part of the Phase-2 feat, **not** a standalone hotfix —
nothing is broken on unix today). Two halves: os-aware manifest naming, and a
bundle-shaped resolve path. Unix behaviour stays byte-identical.

**1a. Manifest name / URL selection.**

- `Channel::manifest_stem` (`release.rs:71-77`) and the two URL builders
  `listing_url` / `listing_sig_url` (`release.rs:244-260`) gain an `os: Os`
  parameter. Stem table (pure, table-tested):
  `(Stable, unix) → "php"`, `(Legacy, unix) → "php-legacy"`,
  `(Stable, Windows) → "php-windows"`, `(Legacy, Windows) → "php-windows-legacy"`;
  sig is always `<stem>.json.minisig`. Same `PHP_LISTING_BASE_URL`.
- `fetch_verified_listing` (`bin/yerdd/src/php_install.rs:155-170`) gains `os: Os`
  and passes it to both URL calls. **Nothing else in fetch/verify changes**: same
  `PHP_LISTING_PUBLIC_KEY`, same prehashed `verify_minisign(pk, sig, body)` at
  `:163` — the producer signs every manifest with the same dedicated key,
  `minisign -S -H` (yerd-php `sign-manifest.sh`).
- Compile-driven caller updates (all already have `(os, arch)` in scope):
  `php_install.rs:193` (`install`), `ipc_server.rs:522,529` (availability),
  `ipc_server.rs:1017,1026` (update apply), `php_updates.rs:35-43` (`fetch_channel`
  gains `os`, threaded from `:58`), plus the test call at `php_install.rs:1023`.

**1b. Bundle-shaped resolve (no union type downstream).**

- Wire struct `BuildEntry` (`release.rs:103-112`): `cli`/`fpm` become
  `Option<FileEntry>` and `bundle: Option<FileEntry>` is added (`#[serde(default)]`
  on all three) so **one** `Listing`/`parse_listing`/`available_minors` stack serves
  both manifest families (a second parallel parse stack would duplicate the schema
  gate, entry selection, and `available_minors` for zero safety gain — the
  strictness moves into the resolvers below). `available_minors` needs no logic
  change (it keys on `minor`/`os`/`arch` only).
- Hoist the entry selection + revision>=1 check out of `resolve_from_listing`
  (`release.rs:271-311`) into a private `select_entry(...) -> Result<BuildEntry, _>`,
  then:
  - `resolve_from_listing` — **signature and `Artifact` shape unchanged**
    (`cli_url/cli_sha256/fpm_url/fpm_sha256` stay as-is, `release.rs:223-242`):
    after selection, require `cli` **and** `fpm` (else `ListingParse` naming the
    missing field — same error variant as today's serde failure, so no IPC/display
    change). Every existing Unix caller compiles and behaves identically.
  - New `resolve_bundle_from_listing(...) -> BundleArtifact` — same params;
    requires `bundle`; `BundleArtifact { version, full_version, revision,
    install_dir_name, bundle_url, bundle_sha256 }` (URL from the manifest `file`
    field verbatim, as today). Called only by the Windows install path (Item 2).
  - New `resolve_build(...) -> Result<(String /*full_version*/, u32 /*revision*/), _>`
    — selection + os-appropriate payload presence check (windows → `bundle`, else
    `cli`+`fpm`), no URLs. The two update call sites that only read
    `full_version`/`revision` switch to it (`php_updates.rs:90`,
    `ipc_server.rs:1053`), which removes any per-OS branching from the update
    paths on all OSes.
  - No new `PhpError` variant; keep revision-0 rejection in `select_entry`.
- Intermediate state after Item 1 alone is sane on Windows: manifests fetch/verify,
  `available_minors` and the update poll work; `install` still fails cleanly
  (`resolve_from_listing` → `ListingParse` on a bundle row) until Item 2 wires
  `resolve_bundle_from_listing` + bundle staging.
- Doc updates in the same change: module doc (`release.rs:1-33`) gains the windows
  manifest section; `Channel` (`:46-57`) and `PHP_LISTING_BASE_URL` (`:84-92`) docs
  mention the four manifest names.
- **Tests** (this is the contract pin; all pure, run un-cfg'd on every OS):
  - `LISTING` fixture (`release.rs:432-455`): **delete** the windows `cli`/`fpm`
    rows (`:451-453`) — the live `php.json` is pure unix again (verified: 0 windows
    rows, `generated_at 2026-08-03T00:57:24Z`).
  - New `WINDOWS_LISTING` (and a one-row `WINDOWS_LEGACY_LISTING`) copied from the
    **live** `php-windows*.json` bundle rows.
  - Rewrite `resolve_from_listing_selects_windows_entry_and_builds_urls`
    (`:503-525`) → `resolve_bundle_from_listing` against `WINDOWS_LISTING`,
    asserting the bundle URL + sha; repoint `available_minors_anchors_windows`
    (`:528-534`) at `WINDOWS_LISTING` (aarch64 stays empty).
  - Extend `listing_urls_point_at_the_signed_manifest` (`:544-562`) with the
    `php-windows.json` / `php-windows-legacy.json` (+ `.minisig`) URLs.
  - Negatives: windows row without `bundle` → `ListingParse` naming the field;
    unix row missing `fpm` → `ListingParse`; `resolve_from_listing` pointed at
    `WINDOWS_LISTING` → `ListingParse` (guards against ever cross-wiring the unix
    resolve to a windows manifest); `resolve_build` covers both os arms.
- Mirror check in the producer repo: none needed — the producer already publishes
  the split manifests (live); this item is the consumer catching up, and the
  fixtures copied from the live bodies pin both repos to the same bytes
  (CLAUDE.md cross-repo note satisfied).

### Item 2 — Windows PHP install + discovery (`bin/yerdd/src/php_install.rs`, `crates/yerd-php/src/version.rs`) and un-cfg the ~14 tests

- **`version.rs` layout fix**: `FPM_BINARY_PATH` non-unix arm (`:17-18`) →
  `&["php-cgi.exe"]`; update the module doc layout block (`:22-27`) to
  `{dirs.data}\php\php-8.3\php-cgi.exe`. `discover_bundled` logic is otherwise
  layout-agnostic and stays.
- **`cli_binary_path`** (`php_install.rs:512-522`): cfg-split — Unix keeps
  `BinaryKind::Cli.install_segments()` (`bin/php`); Windows returns
  `data\php\php-<minor>\php.exe`. `BinaryKind` (`release.rs:183-220`) keeps its
  Unix-only semantics (the Windows flow never uses `archive_member`); note that in its
  doc rather than parameterising it.
- **`install()`/`stage()`** (`php_install.rs:183-225,313-348`): cfg-split the
  resolve+stage segment of `install` (manifests are per-OS, so no runtime union):
  - Unix → `resolve_from_listing` + existing `stage` (two `fetch_and_extract`
    calls), byte-identical.
  - Windows → `resolve_bundle_from_listing` (Item 1b) + new `stage_bundle`:
    single download + SHA-256 verify (same choke point), then a new
    `extract_tree(gz_bytes, staging, url)` that unpacks **every** member:
    `is_safe_member` guard per member (as `service_install::extract_all`,
    `service_install.rs:168-187`), reject non-regular/non-dir entries (no
    symlinks/hardlinks — the bundle has none), create parents, write bytes. No mode
    bits on Windows (nothing to skip — write + `tar` crate are already mode-free
    there). Add a bounded retry (3 attempts, short backoff) on
    `ERROR_SHARING_VIOLATION`-class `io::Error`s per the plan's Defender note —
    implemented as a small helper around the file write, documented, not over-solved.
    `stage_bundle` ends with the same `.yerd-version`/`.yerd-revision` marker
    writes as `stage` (`php_install.rs:339-347`) — hoist them into a shared helper.
  - After the atomic rename in `install()` (`php_install.rs:216-224`), on the bundle
    path append the CA lines to `<final_dir>/php.ini`:
    `curl.cainfo = "<final>\cacert.pem"` / `openssl.cafile = "<final>\cacert.pem"`
    (absolute, quoted; reuse
    `yerd_core::php_settings::sanitize_ca_bundle_path` as `decorate_cli_ini` does at
    `php_install.rs:289-302`). Reinstall replaces the dir so the append is naturally
    idempotent. This discharges the sibling README's "daemon MUST set these at
    install" coordination point.
- **Test fixtures**: add `gzip_tar_tree(members)` beside `gzip_tar_single`
  (`php_install.rs:713-729`); a Windows-shaped signed manifest builder emitting a
  `bundle` row when `current_os_arch()` is Windows (generalise `signed_manifest_for`,
  `:894-916`). `FakeDownloader` (`:861-890`) already answers any `*.json`/
  `*.minisig` URL (so the `php-windows*.json` names route unchanged) but needs a
  `-bundle-` tarball arm beside `-cli-`/`-fpm-`. The existing install tests
  (`install_lays_down_both_binaries_executable` etc.) become platform-branched on
  the fixture side only; assertions check `php.exe`/`php-cgi.exe`/patched
  `php.ini` on Windows.
- **Un-cfg the pending tests**: make `fake_install`/`fake_install_build` in
  `ipc_server.rs` lay down the per-OS layout (`bin/php`+`sbin/php-fpm` vs
  `php.exe`+`php-cgi.exe`), make `listing_body`/`signed_listing`
  (`ipc_server.rs:~4445-4460`) emit `bundle` rows on Windows, fix the SAME
  legacy-routing bug in BOTH `TwoChannelDl` (`:4464`) AND `LegacyOnlyDl::download`
  (`:4504`) — each matches `url.contains("php-legacy.json")`, which misroutes
  `php-windows-legacy.json` (to the stable arm / to the unreachable-error arm
  respectively) — match on `"legacy"` in both, then delete the
  `#[cfg(unix)]` from the 12 tests + 2 helpers (lines 3697, 3749, 3805, 3847, 3877,
  4315, 4340, 4450, 4496/4500, 4515, 4537, 4568, 4599, 4635) and the stale comment at
  `:3692-3696`. Also un-cfg `version.rs`'s discover test via a per-OS layout helper.
- `write_cli_ini` (`php_install.rs:243-287`) already goes through `discover_bundled`
  and needs no change beyond the discovery fix; its `#[cfg(unix)]` scoped test
  (`:786-829`) un-cfgs with the fixture helper.
- Shim functions (`set_default_shim`/`reconcile_shims` non-unix no-ops,
  `php_install.rs:585-705`) stay as they are — **Phase 5**, per plan.

### Item 3 — Job Objects (`crates/yerd-supervise/src/real.rs`) — after ESCALATION ack

- Root `Cargo.toml` `[workspace.dependencies]`: `win32job = "2"` with a comment
  (safe Job Objects wrapper; deps `windows 0.61` + `thiserror 1` already in-lock).
  `crates/yerd-supervise/Cargo.toml`:
  `[target.'cfg(windows)'.dependencies] win32job.workspace = true` (mirror of the
  existing `cfg(unix)` `nix` block).
- `TokioProcessSpawner::spawn` (`real.rs:68-77`), `#[cfg(windows)]` extension:
  1. spawn as today (`kill_on_drop(true)` stays as belt-and-braces);
  2. `Job::create_with_limit_info(&info)` where `info` has
     `limit_kill_on_job_close()`;
  3. `child.raw_handle()` → `job.assign_process(handle as isize)`;
  4. on any job failure: `start_kill()` the child and return the error (**fail
     closed** — a child we cannot contain must not run; doc-comment the rationale).
  Store the job in the child: `TokioChild { inner, pid, #[cfg(windows)] job: Option<win32job::Job> }`.
- `TokioChild::kill` Windows arm (`real.rs:153-158`): for every
  `(signal, protocol)` combination, `self.job.take()` (drop → tree terminated via
  kill-on-close) then `self.inner.kill().await` to reap the direct child handle.
  Remove the `TODO(Phase 2)` comment. `MasterInterrupt` graceful stop is handled a
  layer up (Item 4) *before* `kill` is called; by the time `kill` runs, forced
  termination is intended. Update the trait doc (`traits.rs:30-34`) and the
  `kill_process_group` docs (`real.rs:23-35,57-59`): on Windows the job supersedes
  process-group reaping (dropping the child/job reaps the tree), so `InitGroupReaper`
  (`yerd-services/src/manager.rs:935-955`) now gets its Windows semantics for free.
- **Tests**:
  - `#[cfg(windows)]` unit test: spawn `cmd /C exit 0`, assert the child carries a
    job and `wait` reaps.
  - New `crates/yerd-supervise/tests/job_tree_kill.rs` (`#![cfg(windows)]`) — the
    DoD's orphan test on the CI leg: spawn a leader that provably starts a grandchild
    (e.g. PowerShell: start a detached `ping -n 120 127.0.0.1`, write the grandchild
    PID to a temp file, then sleep), read the PID, `kill(KillSignal::Kill,
    StopProtocol::GroupTerm)` the leader via `ChildHandle`, poll
    `tasklist /FI "PID eq <pid>"` until the grandchild is gone (bounded wait), assert.
    A second case drops the `TokioChild` without killing and asserts the same
    (kill-on-close path).
- No changes to the pure state machine, no Unix diff.

### Item 4 — Services on Windows (`crates/yerd-services`, `bin/yerdd`)

**4a. Executable names.**

- New pure helper in `crates/yerd-services` (e.g. `pure` fn in `version.rs`):
  `fn host_binary_name(base: &str) -> String` appending `.exe` on Windows, built on a
  cross-OS-testable `fn binary_name(base: &str, windows: bool)` (house rule: decisions
  in pure helpers, table-tested on every OS).
- Apply it **inside** the single path joiners so call sites stay untouched:
  `version::server_path` (`version.rs:190-199`) and the `bin/` joins for init/client/
  dump tools — `manager.rs:515` (`init_bin`), `bin/yerdd/src/db_admin.rs:95,246,256`,
  `bin/yerdd/src/services.rs:1233`, `bin/yerdd/src/service_install.rs:153`
  (`stage`'s presence check). Update `server_path`'s unit test with a Windows
  expectation.
- Name divergences via `ServiceDefinition` (additive, defaulted):
  - `fn windows_server_binary(&self) -> Option<&'static str> { self.server_binary() }`
    — `Redis` overrides to `Some("redis-server")` (`service.rs:401`), consumed by a
    tiny `fn server_binary_for_host(def)` helper used at the same joiners.
  - MariaDB init tool: on Windows, probe `bin/` for `mariadb-install-db.exe` then
    `mysql_install_db.exe` in `init_datadir` (`manager.rs:500-540`) — mirroring the
    producer's probe — instead of trusting the static name. Keep the clear
    `ServiceError::Init` when neither exists.
- `discover_installed` (`version.rs:280-321`) picks the fix up via `server_path`.

**4b. Config rendering.**

- `render_my_cnf` (`config_render.rs:62-87`): make the `socket` line conditional —
  plumb an `Option<&Path>` (None on Windows: `mysqld`/`mariadbd` there treat `socket`
  as a named-pipe name; omit rather than depend on it being ignored). The caller
  (`manager.rs:213-215` via `def.render_config`) passes
  `cfg!(unix).then_some(&socket)`-style. Deliberate golden updates: extend the
  cfg(unix) goldens (`config_render.rs:227,297,311`) and add Windows-separator
  variants so the render tests **run on the Windows leg** (they are cfg'd out today
  purely for `\` vs `/`).
- `render_postgresql_conf` is already Windows-correct (TCP-only,
  `unix_socket_directories = ''`).

**4c. Postgres graceful stop (`pg_ctl stop -m fast`), forced-stop fallback.**

- Additive `ServiceDefinition` method (pure — returns a command spec, no I/O):

  ```rust
  /// Windows-only graceful stop: a command to run before forced termination
  /// (no SIGINT exists there). Postgres returns `pg_ctl stop -D <datadir> -m fast`.
  fn graceful_stop_plan(&self, install_dir: &Path, datadir: &Path) -> Option<StdCommand> { None }
  ```

  Postgres impl: program `install_dir/bin/pg_ctl(.exe via 4a)`, args
  `stop -D <datadir> -m fast -t <secs ≈ policy.stop_grace>`, `env_clear` + minimal env.
- `ServiceManager::stop` (`manager.rs:408`) — Windows path only: before driving
  `Event::StopRequested`, when the instance's def has a `graceful_stop_plan`
  (compute `install_dir`/`datadir` from the stored `version` + `datadir_scope`), spawn
  it via `self.spawner` (so it lands in its own job and tests can fake it), wait
  bounded by `policy.stop_grace`. If it exits 0 **and** the supervised child exits
  within the grace window → feed the normal stop flow (child already gone →
  `StopComplete`). Otherwise `tracing::warn!` +append a line to the service log
  (`state/services/postgres/postgres.log`, the file `yerd service logs postgres`
  reads): "pg_ctl stop failed; forcing termination" — then fall through to the
  existing `Action::Kill` path (job termination from Item 3). Unix path: byte-identical
  (the hook is `#[cfg(windows)]`-gated at the call site, not in the trait).
- **Doctor warning — decision point (see §8):** MVP ships the log-line + tracing
  warning only. A first-class `yerd doctor` item requires an additive `StatusReport`
  field + `yerd_doctor::diagnose` rule + wire-stability test (IPC contract touch).
  Recommend deferring that follow-up unless the reviewer wants it in-phase.
- **Tests**: fake-spawner test in `yerd-services` asserting (a) stop invokes the
  graceful plan first on Windows and skips it on Unix (cfg-split assertions), (b) a
  failing `pg_ctl` still results in a kill + `StopComplete`; a unit test pinning the
  exact `pg_ctl` argv.

**4d. `yerd service install` on Windows.**

- `service_install.rs` needs only the 4a name fix (the `bin/<server_binary>` check at
  `:153`) plus the same bounded sharing-violation retry as Item 2 around member
  writes in `extract_all` (`:168-187`). Everything else (tar.gz tree extraction,
  staging + atomic rename) is already OS-clean.
- Un-cfg / extend `yerd-services` tests: `version.rs::server_path_layout` and
  `discover_finds_installed_versions_only` get Windows-name assertions (the fixture
  writes `valkey-server` today — branch the expected name through
  `host_binary_name`).

### Item 5 — PHP runtime: real `WindowsPortBinder` + `php-cgi` supervision (`crates/yerd-platform`, `crates/yerd-php`)

**5a. `WindowsPortBinder` (pull-forward, §0.6.1).**

- `crates/yerd-platform/src/os/windows.rs`: replace the
  `UnsupportedPortBinder as WindowsPortBinder` alias (`:17`) with a real type in the
  **same change** as its full trait impl (never-half-flip): `bind` =
  `TcpListener::bind((127.0.0.1, port))` mapped to `PlatformError::Bind`; `bind_pair`
  = the generic desired→fallback retry documented on the trait
  (`port_binder.rs:44-64`), i.e. the Linux shape (`os/linux.rs:388-392` region)
  without setcap special-casing — direct sub-1024 binds are unprivileged on Windows.
- Tests: `windows_smoke.rs` gains bind-ephemeral + port-readback + AddrInUse cases;
  keep the Phase-1 stub assertions for the traits that remain aliased.

**5b. `php-cgi.exe` pool supervision (`crates/yerd-php`).**

- **Spawn shape** (`build_cmd`, `manager.rs:750-785`) — cfg-split:
  - Unix: unchanged (`--fpm-config`, `process_group(0)`).
  - Windows: `php-cgi.exe -b 127.0.0.1:<port>` (from `cfg.listen`), same
    `-d extension=…` / `-d key=value` prefixing (php-cgi accepts `-d`), scrubbed env
    **plus** `PHP_FCGI_MAX_REQUESTS=0` (§0.6.2 trap) and
    `PHP_FCGI_CHILDREN` unset (fork-based, unsupported on Windows).
  - **Do not pass `-c`**: the bundle's own `php.ini` (exe-dir default) carries
    `extension_dir`, the enabled extension set, and the Item-2 CA lines and must stay
    active. Yerd's per-pool settings (`cfg.ini`, `cfg.directives`, `ca_bundle`)
    instead go into a supplemental ini loaded via `PHP_INI_SCAN_DIR` (absolute path to
    a per-pool dir under `dirs.state`, e.g. `state\php\fpm-8.4\zz-yerd.ini`) — a new
    small pure renderer (reuse `yerd_core::php_settings` pieces), table-tested.
  - `ensure` (`manager.rs:279-385`): cfg-split the config-write step — Unix renders
    `fpm_conf` as today; Windows writes the supplemental ini via the existing
    `atomic_write` and skips the FPM template entirely (`PoolConfig` keeps
    `config_path` as the supplemental-ini path on Windows; document on the field).
    The pure `fpm_conf` renderer and its golden stay untouched.
  - Dump extension name: hoist the `"yerd-dump.so"` join (`manager.rs:310`) into a
    cfg'd `const DUMP_EXT_FILE: &str` (`yerd-dump.dll` on Windows), and mirror in
    `bin/yerdd/src/ext_install.rs` (`so_name` fields `:56-63` → cfg'd
    `yerd-dump.dll`/`pcov.dll`; tmp-file extension at `:211`). The two names must
    move together (manager looks up what ext_install writes) — one shared constant is
    not possible across the crate boundary, so pin each with a test asserting the
    filename, as the shims do for ini names.
- **Bind/rebind race hardening** (`listen.rs:52-80`, `manager.rs:54,410-425`):
  - Keep plan-time retries (`MAX_BIND_ATTEMPTS = 5`) and add a short randomised
    backoff (e.g. 10–50 ms) between attempts on Windows so two colliding planners
    de-synchronise; today the loop retries hot.
  - Close the *post-plan* half of the race: if the spawned `php-cgi` fails its health
    check because the port was re-taken between `drop(bound)` and its own bind, the
    supervisor exhausts restarts on the **same baked port**. Windows-only: on
    `PhpError::HealthCheckTimedOut`/`PermanentFailure` from the first drive in
    `ensure`, replan a fresh port once and retry the whole ensure (bounded to one
    replan; documented).
  - Tests: fake-binder test asserting `plan` returns `TcpLoopback(127.0.0.1:<port>)`
    on Windows and never binds on Unix (un-cfg the existing `#[cfg(unix)]` plan test
    by table-driving both branches with a stub binder); manager test with a fake
    spawner + fake probe pinning the replan-once behaviour.

### Item 6 — FastCGI end-to-end + DoD wiring

- **Hermetic probe test (all OSes, runs on the Windows leg):** in
  `fastcgi_probe.rs` tests, bind a real `tokio::net::TcpListener` on
  `127.0.0.1:0`, answer one `FCGI_GET_VALUES` with a valid version-1 header, and
  probe via `Listen::TcpLoopback` — this genuinely exercises the Windows TCP branch
  (`:37-41`) in CI rather than only the `duplex` streams.
- **Real-FPM exercise is a manual DoD gate on this machine** (CI cannot download
  PHP): `cargo run -p yerdd`, `yerd php install 8.4` (consumes the live bundle),
  confirm pool up (`php-cgi.exe` listening), FastCGI probe green,
  `yerd-dump.dll` loaded (`php -m` via the pool / dumps window smoke), proxy
  `Backend::PhpFpmTcp` forward works, `yerd service install postgres 18.4` +
  start/stop (pg_ctl path) + `yerdd` shutdown with `tasklist` showing zero leftover
  `php-cgi.exe`/`postgres.exe`/`mysqld.exe`. Record results in the PR description.
- The automated orphan guard on the CI leg is Item 3's `job_tree_kill.rs`.

### Item 7 — Docs, instruction files, stale comments

- `crates/yerd-supervise/src/real.rs` TODO + doc updates (done in Item 3);
  `traits.rs:30-34` protocol doc gains the Windows sentence.
- `.github/instructions/yerd-php.instructions.md`: the "No Unix sockets for PHP-FPM on
  Windows — use TCP loopback" trap gains the php-cgi reality (no FPM binary on
  Windows; `php-cgi.exe -b`, supplemental-ini mechanism).
- `docs/developer/crates/yerd-php.md:319` (build_cmd description) and
  `docs/developer/crates/yerd-supervise.md:93` (process-group doc) get the Windows
  arms. `yerd-services` docs likewise for exe names + pg_ctl stop.
- `config_render.rs` test-scoping comments (`:224-227`) are now stale — remove with
  the un-cfg.
- CI: no workflow change required (Windows leg exists). Optionally extend the Phase-1
  "non-vacuous green" guard to also `--list`-count the un-cfg'd `ipc_server`
  install tests; cheap insurance that they never regress to `cfg(unix)`.

---

## 7. Ordering & compile checkpoints

```
Item 1  (windows manifests + parse)  ── FIRST within the feat (pure-crate work); NOT a
                                        standalone release — unix is unbroken today (§0.1)
Item 2  (bundle install + discovery) ── needs 1 (resolve_bundle_from_listing + URL selection)
Item 3  (Job Objects)                ── independent of 1/2; needs ESCALATION ack (new dep)
Item 4  (services: names, pg_ctl)    ── 4a/4b/4d independent; 4c's forced-stop fallback
                                        assumes Item 3's kill actually reaps the tree
Item 5  (PortBinder + php-cgi)       ── 5a before 5b; 5b needs 2 (php-cgi discovered)
                                        and benefits from 3 (worker teardown)
Item 6  (FastCGI e2e + manual gate)  ── needs 2+5 (and 4 for the postgres DoD line)
Item 7  (docs/comments)              ── with or after the item it documents
```

Every item independently passes the full gate on ubuntu / macos / windows.

## 8. IPC-contract / pure-crate boundary flags (per CLAUDE.md, surfaced not worked around)

1. **Doctor warning for forced Postgres stop** (Item 4c): a real `yerd doctor` entry
   requires an additive `StatusReport` field (IPC wire change + wire-stability test +
   GUI surface). MVP ships log-line + tracing only; the IPC-additive follow-up is
   explicitly deferred — say the word and it lands in-phase instead.
2. **`ServiceDefinition::graceful_stop_plan`** is designed to stay pure (returns a
   command spec; the manager does the spawning through the injected `ProcessSpawner`),
   so no I/O enters the trait layer.
3. **`win32job`** enters the dependency graph of a bottom-layer crate
   (`yerd-supervise`) — Windows-target-only, two already-locked transitive deps; no
   `no_runtime_deps` guard is weakened (those check `anyhow`/`reqwest`/OpenSSL and
   tokio/time singletons).
4. **Item 1 consumes the Windows side of a signed cross-repo contract** — the
   producer moved first and the split manifests (`php-windows.json` /
   `php-windows-legacy.json` + `.minisig`) are live; no producer change needed. The
   new fixtures are copied from the live manifest bodies so the two repos are
   pinned to the same bytes, and the same dedicated minisign key + prehash covers
   all four manifests (verified in `yerd-php/scripts/sign-manifest.sh`).
5. **`yerd-dump.dll` on-disk naming** (Item 5b) pairs `bin/yerdd/src/ext_install.rs`
   with `crates/yerd-php/src/manager.rs` — kept in lockstep by filename-pinning tests
   on both sides (no cross-repo change: the sibling already publishes the DLLs).

## 9. Deviations from WINDOWS_PLAN.md (flag for confirmation)

1. **No `.zip` extraction path is added.** Verified that no Phase-2 artifact is a zip
   (§0.4); the `zip` crate stays available for the phase that first needs it
   (likely Phase 6 self-update). Chmod-skip already exists everywhere relevant.
2. **"Cfg-gate the `process_group(0)` calls" is already done** (Phase 1 compile
   floor); Phase 2 instead supplies the Windows equivalent via Job Objects.
3. **`yerd-mail` needs no supervision work** — it is an in-process tokio SMTP server,
   not a child process (§0.7). It gets exercised by the daemon simply running.
4. **`WindowsPortBinder` is pulled forward from Phase 3** (§0.6.1) because FPM's
   ephemeral bind depends on it; Phase 3 retains the 80/443 validation + doctor work.
5. **"exercise + harden `listen.rs`" grew into a php-cgi supervision item** (Item 5b):
   the published Windows artifact has no FPM binary, so "FPM comes up per site config"
   is only achievable by spawning `php-cgi.exe` — the bundle-layout/discovery
   resolution the plan asked this phase to make.
6. **Known MVP limitation to record in the PR/docs:** one `php-cgi.exe` process per
   PHP version = **one concurrent PHP request per version** (php-cgi cannot
   pre-fork on Windows). Herd solves this with a small worker pool; tracked as a
   post-MVP TODO (spawn N php-cgi workers on N ports + round-robin in the proxy
   backend), not Phase-2 scope.
7. **Redis/MariaDB Windows artifacts are not yet published** by `yerd-services`
   (§0.3); installs correctly report `VersionUnavailable` until then. When the
   sibling publishes, the Item-4a name overrides (`redis-server.exe`, probed MariaDB
   init tool) are the already-mirrored consumer side.

## 10. New / changed dependencies

| Dep | Where | Why | Notes |
|---|---|---|---|
| `win32job = "2"` | workspace table + `yerd-supervise` `[target.'cfg(windows)']` | safe Job Objects API | ESCALATION Option A; transitive deps (`windows 0.61`, `thiserror 1`) already in `Cargo.lock`; pin per house style |

Nothing else. (`zip` already present and unused this phase; no `windows-sys` direct
dep; no `unsafe` anywhere.)

## 11. Definition of done (restating the plan, made checkable)

- [ ] `cargo fmt/clippy -D warnings/test --workspace` green on ubuntu-22.04,
      ubuntu-22.04-arm, macos-14, windows-latest.
- [ ] **Live-manifest contract pinned:** the pure-unix `php.json` fixture and the
      `WINDOWS_LISTING` bundle fixture (both copied from the live manifests) parse
      and resolve on all OSes (Item 1 tests), including the cross-wiring negative
      (`resolve_from_listing` against a bundle manifest → `ListingParse`); on
      Windows the daemon fetches `php-windows.json` / `php-windows-legacy.json`
      (+ `.minisig`) and verifies them with the unchanged minisign path.
- [ ] On this Windows machine: `yerd php install 8.4` lays down the bundle,
      `php.ini` carries the absolute `cacert.pem` lines, `discover_bundled` finds
      `php-cgi.exe`, the pool binds `127.0.0.1:<port>` and the FastCGI probe passes.
- [ ] `yerd service install postgres 18.4` + start + stop on Windows: stop goes
      through `pg_ctl stop -m fast`; killing `pg_ctl` support (rename it away) still
      stops the service via forced job termination with the logged warning.
- [ ] `yerd-supervise/tests/job_tree_kill.rs` **executes on the Windows CI leg** and
      proves a grandchild dies with the leader (kill and drop paths).
- [ ] The 12 `ipc_server.rs` install/discovery/poll tests + helpers, the
      `version.rs` discover test, the `listen.rs` plan test, and the
      `config_render` goldens run un-cfg'd on Windows (verify via `--list`, as
      Phase 1's CI guard does for lifecycle).
- [ ] `yerdd` shutdown on Windows leaves zero `php-cgi.exe` / `mysqld.exe` /
      `postgres.exe` / init-tool processes (manual `tasklist` sweep recorded in the
      PR; automated tree-kill guard in CI).
- [ ] macOS/Linux behaviour byte-identical outside the deliberately-regenerated
      `render_my_cnf` golden (Item 1 leaves the unix wire shape, resolve results,
      and manifest URLs unchanged; only internal signatures gain an `Os` parameter
      and the update paths switch to the behaviour-equivalent `resolve_build`).
- [ ] Instruction-file and developer-doc updates land with the code they describe.
