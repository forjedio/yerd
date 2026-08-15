# Phase 3 Implementation Plan — Serve sites: ports, TLS trust, proxy end-to-end

Working doc (not committed). Companion to `WINDOWS_PLAN.md` Phase 3 and successor to
`PHASE2_PLAN.md` (Phases 1–2 are committed as `ed9add2` / `1e0de5a`: paths, IPC,
Windows CI leg, Job Objects, `php-cgi.exe` pools, services, FastCGI-over-TCP, and a
**real `WindowsPortBinder`** pulled forward). Everything below was verified against the
actual code on this Windows machine on 2026-08-03, including a green
`cargo check -p yerd-platform -p yerd-doctor -p yerd-proxy` and a source-level audit
of `schannel 0.1.29` in the local cargo registry.

Locked decision honoured throughout: **cert store = CurrentUser Root** (WINDOWS_PLAN
"Decisions", item 1). Firefox/NSS on Windows is explicitly out of scope (Phase 6 TODO).

---

## 0. Ground truth (verified, not assumed)

### 0.1 The `TrustStore` trait surface the Windows impl must match exactly

`crates/yerd-platform/src/trust_store.rs:130-216`. Eight methods; six must be
written (two have defaults):

```rust
fn install_system(&self, ca_pem: &str, fp: &CaFingerprint) -> Result<(), PlatformError>;
fn uninstall_system(&self, fp: &CaFingerprint) -> Result<(), PlatformError>;
fn is_present_system(&self, fp: &CaFingerprint) -> Result<bool, PlatformError>;
fn is_trusted(&self, ca_path: &Path, fp: &CaFingerprint) -> Result<bool, PlatformError>; // defaulted (Unsupported)
fn install_firefox_nss(&self, ca_path: &Path) -> Result<NssOutcome, PlatformError>;
fn uninstall_firefox_nss(&self) -> Result<NssOutcome, PlatformError>;
fn browser_ca_trust(&self, fp: &CaFingerprint) -> Result<BrowserCaTrust, PlatformError>; // defaulted (Unsupported)
fn system_root_bundle(&self) -> Result<Option<String>, PlatformError>;
```

- **The CA arrives as PEM** at every call site: `install_system` takes `ca_pem: &str`;
  the on-disk artifact is `{data}/ca.cert.pem` (`bin/yerdd/src/startup.rs:135`).
  The identity is `CaFingerprint` = **SHA-256 over the DER body**
  (`trust_store.rs:39-47`, same definition as `yerd_tls`'s `fingerprint_sha256`).
  PEM→DER conversion already exists as a pure helper:
  `crate::pure::pem_match::first_cert_der` (used by `os/macos.rs:274`), plus
  `sha256`, `der_to_pem` (`pure/pem_match.rs:58,69,113`). schannel wants DER
  (`CertContext::new(&[u8])`) but also accepts PEM directly (`CertContext::from_pem`);
  we will convert via `first_cert_der` ourselves so the fingerprint check runs on the
  **exact DER bytes we import** (the `mac_trust.rs` integrity-gate pattern).
- Error type: `PlatformError::TrustStore { reason: TrustStoreErrorReason }`
  (`error.rs:49-54,123-150`). `TrustStoreErrorReason::SystemApi(String)` is the
  catch-all for OS-API failures (macOS uses it for every `security-framework` error);
  both enums are `#[non_exhaustive]`, so no new variant is required.
- Op tags already exist: `ops::INSTALL_CA` / `UNINSTALL_CA` / `IS_PRESENT_SYSTEM` /
  `IS_TRUSTED` (`error.rs:222-234`).
- **Key structural fact:** on macOS/Linux `install_system`/`uninstall_system` never do
  the work — they return `PlatformError::NeedsHelper` and the *CLI under sudo*
  (`bin/yerd/src/elevate.rs:269-273`) drives `yerd-helper` with
  `HelperInvocation::InstallCa/UninstallCa`. The daemon never installs trust. On
  Windows, CurrentUser Root needs **no elevation**, so the Windows impl performs the
  operation directly and returns `Ok(())` — no helper, no `HelperInvocation` change,
  no IPC change. The trait doc ("always returns NeedsHelper in Phase 1") gets a
  Windows sentence.
- Call sites that light up once the impl is real (all already compiled on Windows):
  - `bin/yerdd/src/ipc_server.rs:753-761` — `is_trusted` feeds
    `StatusReport.ca.trusted_system` (drives doctor's `CaNotTrusted` + GUI status).
  - `bin/yerdd/src/ipc_server.rs:1384-1388` and `startup.rs:138-144` —
    `system_root_bundle` feeds `build_php_ca_bundle` (`{data}/cacert.pem`, what makes
    PHP trust the Yerd CA). Today Windows aliases the unsupported stub, which returns
    `Ok(None)` (`os/unsupported.rs:114-116`) → the bundle machinery is inert and PHP
    HTTPS to `.test` fails cert verification.
  - `browser_ca_trust`/NSS via `.ok()` → `None`/error paths — safe to leave
    Unsupported (see 0.5).

### 0.2 Where trust is *triggered* — and why it must be an interactive process

- CLI: `yerd elevate [trust] [--undo]` exists (`bin/yerd/src/cli.rs:220-229`,
  `ElevateTarget::Trust` at `:783-785`). The non-Unix `run_elevate` is a stub that
  prints "elevate is only supported on Unix" and exits 78 (`elevate.rs:14-22`).
  The Windows IPC client transport is done (Phase 1): `transport::exchange` has a
  `#[cfg(windows)]` named-pipe arm (`bin/yerd/src/transport.rs:41-65`), and
  `Request::DaemonInfo` returns `ca_path` + `ca_fingerprint` (used by `fetch_facts`,
  `elevate.rs:449-498`).
- GUI: macOS has an **in-process, per-user** trust path
  (`apps/yerd-gui/src-tauri/src/mac_trust.rs`, commands `trust_ca`/`untrust_ca` in
  `commands.rs:767-797`, gated `platform === "macos"` in
  `apps/yerd-gui/src/components/EnvironmentCard.vue:200,257`). Windows CurrentUser
  Root is the exact analogue; an in-app arm is possible but **optional** (Item 7).
- **Windows shows a confirmation dialog for CurrentUser Root adds *and* deletes**
  (the crypt32 "You are about to install a certificate…" / "…DELETE the following
  certificate…" security warning). It cannot be suppressed programmatically, needs no
  UAC, and requires an **interactive desktop** — in a non-interactive session the add
  fails (`ERROR_CANCELLED`-class error) instead of hanging. Three consequences:
  1. the install/uninstall must run in the **CLI or GUI process**, never in `yerdd`
     (Phase 5 turns the daemon into a session-0 service with no desktop);
  2. CI tests must never touch the real Root store (Item 2's test strategy);
  3. `certutil -user -addstore` pops the *same* dialog — no option avoids it.
  This matches WINDOWS_PLAN's "one-time confirmation dialog but no UAC" line.
- Browsers: Edge/Chrome/Chromium on Windows read the Windows (CurrentUser +
  LocalMachine) Root stores → trusting in CurrentUser Root is sufficient for them.
  Firefox keeps its own NSS profile store (unless `security.enterprise_roots.enabled`)
  → documented manual-trust note, Phase 6 TODO, per the master plan.

### 0.3 `schannel` — the safe cert-store crate (audited from vendored source 0.1.29)

- **Not currently in `Cargo.lock`** (grepped — zero hits; rustls does not pull it).
  Present in the local registry at
  `~/.cargo/registry/src/…/schannel-0.1.29` from another project, so the audit below
  is against real source, not docs.
- Metadata: MIT, `rust-version = 1.71` (workspace MSRV is 1.77 — fine), **sole
  dependency `windows-sys = "0.61"`** — already resolved in our lock (`0.61.2`), so
  the net-new lock entry is `schannel` itself. Exactly the `win32job` shape from
  Phase 2.
- Safe public API coverage, mapped to the four operations we need
  (`src/cert_store.rs`, `src/cert_context.rs` — all `unsafe` is internal to the
  crate; our code stays `forbid(unsafe_code)`-clean):

  | Need | Safe schannel API | Win32 underneath |
  |---|---|---|
  | open CurrentUser "Root" (read/write) | `CertStore::open_current_user("Root")` (`cert_store.rs:105`) | `CertOpenStore(CERT_STORE_PROV_SYSTEM_W, CERT_SYSTEM_STORE_CURRENT_USER)` — default access = read/write, maximum allowed |
  | DER → context | `CertContext::new(&der)` (`cert_context.rs:91`) | `CertCreateCertificateContext` |
  | add | `store.add_cert(&cx, CertAdd::ReplaceExisting)` (`cert_store.rs:196`) | `CertAddCertificateContextToStore` (this is the call that raises the Root confirmation dialog) |
  | probe | `store.certs()` iterator (`:185`) + `cx.to_der()` → `pem_match::sha256` compare | `CertEnumCertificatesInStore` |
  | delete | `cx.delete()` (`cert_context.rs:307`) | `CertDeleteCertificateFromStore` |
  | read LocalMachine Root (for `system_root_bundle`) | `CertStore::open_local_machine("Root")` (`:131`) — open + enumerate need no admin; only writes would | same, `LOCAL_MACHINE` location |
  | hermetic tests | `Memory::new()` in-memory store (`cert_store.rs:361-433`) | `CERT_STORE_PROV_MEMORY` |

  **Every operation Phase 3 needs is safe-API-supported; nothing requires dropping to
  FFI.** Caveats found in source: (a) the `Certs` iterator hands out *cloned*
  (duplicated) contexts — still, delete after finishing enumeration, not mid-iteration
  (`CertEnumCertificatesInStore` frees the previous context each step); (b)
  `CertContext::from_pem` contains an internal `assert!` — dependency code, not
  subject to our clippy denies, and we use `CertContext::new(der)` anyway.
- **native-tls guilt-by-association check:** schannel is the crate `native-tls`
  builds on for Windows, but the dependency direction means adding schannel pulls in
  **nothing** TLS-related. The dep-graph guards
  (`crates/yerd-platform/tests/no_runtime_deps.rs` forbids
  `tokio/anyhow/reqwest/openssl/openssl-sys/native-tls`; likewise `yerd-proxy`,
  `yerd-php`, `yerd-tls`) stay green — `schannel` is not on any forbidden list and we
  use only its `cert_store`/`cert_context` modules, never `tls_stream`. The "TLS is
  rustls + rcgen" hard rule is untouched.

### 0.4 `certutil.exe` — the no-dep fallback, precisely assessed

- Install: `certutil -user -addstore Root <pem-or-der-file>` → CurrentUser Root, **no
  elevation**, accepts PEM or DER, same confirmation dialog, reliable exit code.
  Absolute-path spawn from `%SystemRoot%\System32\certutil.exe` would follow the
  existing `whoami.exe` precedent (`os/windows.rs:241-245`).
- Uninstall: `certutil -user -delstore Root <id>` where `<id>` is a **SHA-1**
  thumbprint or a subject CN substring. Our identity is SHA-256-of-DER; the workspace
  has `sha2` but no `sha1` dep, and CN-matching risks deleting a foreign cert (or
  missing a rotated Yerd CA). Lossy.
- Probe: `certutil -user -store Root` output is **localized free text** (German/French
  Windows breaks any parser); there is no structured output mode. Fragile.
- Verdict: covers install adequately, probe and uninstall poorly. See ESCALATION.

### 0.5 What the Windows `TrustStore` should do per method (design facts)

- `is_trusted`: on Windows, presence in the Root store **is** trust (no separate
  trust-settings layer like macOS) → delegate to `is_present_system`, exactly the
  Linux comment/shape (`os/linux.rs:282-287`).
- `browser_ca_trust`: leave the **default** (`Unsupported`) body. Call sites use
  `.ok()` → `StatusReport.ca.browser_trust = None` → doctor emits nothing
  (`yerd-doctor/src/lib.rs:100-119` matches only `Some(..)`). Correct for Windows:
  Chromium-family browsers follow the system store (covered by `trusted_system`), and
  Firefox/NSS is the locked Phase-6 non-goal. `nss_exec.rs` is Unix-only and stays so.
- `install_firefox_nss`/`uninstall_firefox_nss`: keep returning
  `Err(Unsupported { INSTALL_FIREFOX_NSS/… })` like the stub (`os/unsupported.rs:99-109`)
  — the Windows CLI arm (Item 3) simply won't call the browser-trust follow-up that
  `sudo yerd elevate` runs on Unix (`elevate.rs:126-128,139-175`).
- `system_root_bundle`: implement via schannel — enumerate **LocalMachine Root +
  CurrentUser Root** read-only, `pem_match::der_to_pem` each, concatenate (macOS
  doesn't dedupe either, `os/macos.rs:322-370`); `Ok(None)` when empty. This is a
  small **addition beyond the master plan's letter** (flagged in §9) but it is what
  makes `build_php_ca_bundle` → `{data}/cacert.pem` → `php_trusts_ca` →
  `yerd doctor fix`'s `RebuildPhpCaBundle` work on Windows; without it, PHP's HTTPS
  calls to `.test` sites fail (cURL error 60) with no doctor visibility
  (`php_trusts_ca` stays `None` because `state.php_ca_bundle` is `None`).
- `windows_smoke.rs:48-64` currently asserts the trust methods return `Unsupported` —
  those assertions must be replaced **in the same change** that flips the alias, or
  the Windows CI leg fails mid-item.

### 0.6 Ports: what is actually left for Phase 3

- `WindowsPortBinder` is **done** (Phase 2 pull-forward): direct binds including
  sub-1024 (unprivileged on Windows), generic desired→fallback retry via
  `pure/port_plan` (`os/windows.rs:79-217`), smoke-tested
  (`windows_smoke.rs:89-115`). Nothing to add.
- `PortRedirector` on Windows is still the `unsupported` alias (`os/windows.rs:21`),
  whose overrides return `None` for **both** `is_active` and `foreign_web_listener`
  (`os/unsupported.rs:211-225`) → `StatusReport.foreign_web_listener = None` → the
  doctor's "Another process is using port 80/443" finding (`lib.rs:318-328`) can
  never fire on Windows. But the **trait default** `foreign_web_listener`
  (`port_redirect.rs:48-53`: HTTP-probe 80 for the proxy's `Server: yerd` marker,
  else TCP-probe 80/443) is documented as "correct on every OS where Yerd serves over
  loopback" — Windows qualifies. So the remaining port work is exactly:
  1. a real `WindowsPortRedirector` that inherits that default and returns
     `is_active = None` (no redirect concept — Windows direct-binds), and
  2. Windows-aware doctor copy: today's remedies say `sudo yerd elevate ports`
     (`lib.rs:326,363,393,419,430-435`) and "80/443 need elevation" — wrong on
     Windows, where 80/443 conflicts mean **IIS/W3SVC, an `http.sys` listener
     (`netsh http show servicestate`), or legacy Skype**, and the fix is to stop the
     squatter and restart the daemon (elevation cannot help).
  No new `DiagnosisCode` and **no IPC change**: `ForeignWebListener`,
  `WebPortsUnbound`, `PortFallback`, and the `web_unbound`/`foreign_web_listener`
  report fields all exist. Doctor tests at `lib.rs:668,673,675,755` pin the
  `sudo yerd elevate ports` strings and must be branched with the copy.
- Active `urlacl`-reservation *enumeration* (spawning `netsh`) is deliberately **not**
  built — the passive listener probe plus a message that names `netsh http show
  servicestate` is the minimal, sufficient doctor check (§9).

### 0.7 Proxy + site paths: state of play

- `yerd-proxy` is pure hyper/tokio-rustls; certs come from the in-memory
  `DaemonCertStore` (rustls/rcgen) — no OS trust involvement, **no code change
  expected** (confirmed: no cfg-gated OS code in `src/`).
- The FastCGI param builder is already Windows-path-aware:
  `SCRIPT_NAME` normalizes `\` → `/` (`pure/cgi_params.rs:83`), `SCRIPT_FILENAME` /
  `DOCUMENT_ROOT` are native `document_root.join(...)` (PHP on Windows accepts both
  separators), and one integration test already tolerates `\`
  (`tests/integration_http.rs:849`). What's missing is a **pinning unit test** for
  the separator behaviour (the master plan's "classic bug" test) — Item 6.
- Proxy integration tests run on the Windows CI leg except four
  symlink-fixture tests (`integration_http.rs:1137,1200,1266,1330` —
  `std::os::unix::fs::symlink`); creating symlinks on Windows CI needs Developer
  Mode, so leaving them `#[cfg(unix)]` is a deliberate, commented gap (Laravel
  `storage:link` behaviour on Windows is a post-MVP validation TODO).
- Site config → router → proxy flows `PathBuf`s end-to-end (`Site::linked(name,
  PathBuf, PhpVersion)`); no string-splitting on `/` was found in the route.

### 0.8 Site creation: one real Unix-ism blocks the GUI flow

- The streamed job runner is portable (`create_site/mod.rs:322,405` — cfg'd
  process-group/kill arms already exist; Job Objects from Phase 2 do the real reaping).
- PATH composition is portable (`std::env::join_paths`,
  `create_site/laravel.rs:240-247`); external-tool discovery has non-Unix arms
  (`tools/external.rs:110-113,148-152` — login-shell PATH capture degrades to the
  process env, fine).
- The scaffold entry point is a direct `php.exe <installer.phar> new …` spawn
  (`laravel.rs:108-123`) — portable.
- **Blocker:** `build_job_bin` — the per-job `bin/` that pins `php`/`composer` for the
  installer's *nested* invocations — is a symlink + `#!/bin/sh` wrapper on Unix and a
  hard `Err("site creation is not yet supported on this platform")` off Unix
  (`laravel.rs:305-312`). Windows needs `.cmd` wrappers (Item 5). Nested resolution
  goes through PHP `proc_open` → `cmd.exe`, which resolves `.cmd` via `PATHEXT` —
  unlike Rust's `Command::new`, so wrappers work for the nested layer while the outer
  spawn stays a real `.exe`.
- WordPress flow (`wordpress.rs`) has no cfg-gated code (wp-cli phar via the same
  `php.exe` runner) — expected portable; verified in the manual smoke.
- `{data}/bin` tool shims (`tools/mod.rs:296-301`) are a documented non-Unix no-op —
  **Phase 5** (`.cmd` shim delivery), not Phase 3; the per-job bin below is job-local
  and does not touch that mechanism.

### 0.9 GUI

- `usePlatform` already exposes `isWindows` (Phase 1;
  `apps/yerd-gui/src/composables/usePlatform.ts`).
- The trust UI (`EnvironmentCard.vue`) gates in-app trust on `platform === "macos"`
  and otherwise shows "In-app elevation isn't available on this platform yet — use
  `yerd elevate` in a terminal for now" (`:338`) — which becomes *true and usable* on
  Windows after Item 3 (minus the `sudo`).
- Sites list / create / PHP-version-per-site are IPC-driven views with no OS
  branching found; the create flow's only blocker is 0.8's `build_job_bin`.

---

## ESCALATION / DECISION — CA-store mechanism (ack before Item 2)

**Question:** how to install/probe/uninstall the Yerd CA in the CurrentUser Root
store under the workspace-wide `unsafe_code = "forbid"`?

**Answer: no `unsafe` is needed anywhere in Phase 3.** Two safe paths exist; this is
a dependency-ack decision (the `win32job` precedent), not a forbid-lift escalation.

**Option A — `schannel` crate (recommended).**
Safe wrappers for every needed op, verified from source (§0.3): open CurrentUser
"Root" read/write, add (`CertAdd::ReplaceExisting`), enumerate+fingerprint probe,
delete, plus read-only LocalMachine/CurrentUser enumeration that makes
`system_root_bundle` (PHP CA bundle) work — and an in-memory store for hermetic,
dialog-free CI tests.
*Pros:* typed, structured, locale-independent; probe/uninstall by exact SHA-256-of-DER
identity (matches `CaFingerprint` exactly); one net-new lock entry (sole dep
`windows-sys 0.61` already locked); MIT; MSRV 1.71; ~2 relevant source files,
auditable in an hour; maintained (the `native-tls` ecosystem sits on it).
*Cons:* a new third-party dep in `yerd-platform`'s Windows graph; the crate's name
suggests a TLS stack (we use only cert-store modules — no TLS, rustls rule intact,
`no_runtime_deps` guards unaffected, §0.3).

**Option B — `certutil.exe` subprocess (safe, zero deps).**
`-user -addstore Root` installs fine (no elevation, same dialog, absolute-path spawn
per the `whoami` precedent).
*Cons (disqualifying as the primary):* uninstall keys on SHA-1/subject-CN, not our
SHA-256 identity (no `sha1` dep in the workspace; CN matching can delete a foreign
cert or miss a rotated CA); the presence probe requires parsing **localized** text
output; and `system_root_bundle` would still have no implementation, leaving PHP
unable to trust the CA. Would also sit awkwardly next to
`yerd-platform.instructions.md`'s "no shelling out to perform privileged work"
spirit, even though this op is unprivileged.

**Option C — raw Win32 FFI (`windows-sys`).** Requires `unsafe`; the workspace
forbids it. **Not needed** — Option A covers 100% of the surface. Rejected without a
separate decision, per the Phase-2 precedent.

**Recommendation: Option A.** Pin `schannel = "0.1"` in `[workspace.dependencies]`
with a comment (cert-store access only, never TLS; sole transitive dep already in
lock), consumed as `[target.'cfg(windows)'.dependencies]` of `yerd-platform` —
mirroring both the macOS `security-framework` block and Phase 2's `win32job` entry.
If vetoed: Option B for install only, with uninstall-by-CN + localized-probe caveats
documented and `system_root_bundle` left as `Ok(None)` (PHP trust gap becomes a
tracked TODO). **Item 2 waits on this ack; Items 1, 5, 6 do not.**

Confirmations requested alongside the ack (both verified true to the best of this
investigation, restated for the record):
- CurrentUser Root install needs **no elevation** — the OS gates it with the
  per-user confirmation dialog instead of an ACL; `certutil -user` behaves
  identically. Phase 3 therefore lands before Phase 4's UAC helper, as planned.
- Edge/Chrome/Chromium trust the CurrentUser Root store; Firefox does not
  (Phase 6 TODO, locked out of scope).

---

## Implementation checklist

Ordered; each item ends at a compiling workspace on all three OSes
(`cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings &&
cargo test --workspace`). Unix behaviour stays byte-identical throughout (no Unix
code path is touched except doctor message helpers gaining cfg-branches).

### Item 1 — Real `WindowsPortRedirector` + Windows doctor copy (no dep needed)

- `crates/yerd-platform/src/os/windows.rs`: add `WindowsPortRedirector` (unit struct,
  `const fn new()`, same shape as the others) implementing `PortRedirector` with only
  `is_active() -> None` written (doc: Windows direct-binds via `WindowsPortBinder`;
  there is no redirect to be "active", but the trait **default**
  `foreign_web_listener` probe applies — that is the point of the type).
  `redirect_targets`/`lan_redirect_targets` keep their `None` defaults (pf is
  macOS-only). Remove `UnsupportedPortRedirector as WindowsPortRedirector` from the
  `pub use` (`os/windows.rs:20-26`) **in the same change** (never-half-flip);
  `os/mod.rs` re-exports are name-based and need no edit.
- `crates/yerd-doctor/src/lib.rs` — cfg'd copy branches (precedent:
  `port_fallback_remedy`'s `cfg!(target_os = "macos")` at `:430-436`; the crate is
  pure over `StatusReport` and compiled per-OS, so `cfg!` string selection stays pure):
  - `ForeignWebListener` (`:318-328`): Windows detail/remedy — "Stop the other web
    server: IIS/W3SVC (`net stop w3svc`), an `http.sys` app (`netsh http show
    servicestate`), or legacy Skype's port-80 fallback — then restart the Yerd
    daemon." (no `sudo yerd elevate ports`, which does not exist there).
  - `PortFallback` (`:359-371`): Windows detail drops "need elevation" ("80/443 were
    busy when Yerd started; serving on the rootless ports"), remedy = "free the port,
    then restart the daemon" via a Windows arm in `port_fallback_remedy`.
  - `trust_findings` (`:97`): `CaNotTrusted` remedy `sudo yerd elevate trust` →
    `yerd elevate trust` on Windows (small helper à la `port_fallback_remedy`).
  - `WebPortsUnbound`/`DnsPortUnbound` copy is already OS-neutral — leave.
- Tests: branch the string assertions at `lib.rs:668,673,675,755` (they run on the
  Windows CI leg and would pin the Unix copy); add Windows-side expectations for the
  three changed messages. `windows_smoke.rs`: add a `port_redirector` test —
  `is_active()` is `None`, `foreign_web_listener()` is `Some(_)` (value depends on
  the host; assert `is_some()`, not the bool), `redirect_targets()` is `None`.
- No IPC change (fields + codes all pre-exist, §0.6).

### Item 2 — `WindowsTrustStore` via schannel (after ESCALATION ack)

- Root `Cargo.toml` `[workspace.dependencies]`:
  `schannel = "0.1"` with a comment (safe CurrentUser/LocalMachine cert-store access
  for the Windows `TrustStore`; **cert store only, never TLS** — rustls rule intact;
  sole dep `windows-sys 0.61` already in lock).
  `crates/yerd-platform/Cargo.toml`:
  `[target.'cfg(windows)'.dependencies] schannel = { workspace = true }` (beside the
  existing macOS `security-framework` block).
- `crates/yerd-platform/src/os/windows.rs`: replace the
  `UnsupportedTrustStore as WindowsTrustStore` alias with a real unit struct + full
  `TrustStore` impl **in the same change**, updating the module doc (`:1-7`):
  - Private core helpers written against `&schannel::cert_store::CertStore` so the
    same code runs on the real "Root" store and on `Memory` in tests:
    `add_der(store, der)`, `find_by_fp(store, fp) -> Vec<CertContext>`,
    `store_root_pem(store) -> String`.
  - `install_system(ca_pem, fp)`: `pem_match::first_cert_der(ca_pem.as_bytes())`
    (else `TrustStore { SystemApi("CA PEM has no certificate") }`, the macOS wording,
    `os/macos.rs:274-276`) → **verify `pem_match::sha256(&der) == *fp.as_bytes()`
    on the exact bytes to be imported** (else `SystemApi("CA PEM does not match the
    expected fingerprint")` — the `mac_trust.rs`/helper integrity gate) →
    `CertStore::open_current_user("Root")` → `CertContext::new(&der)` →
    `add_cert(.., CertAdd::ReplaceExisting)`. Map every `io::Error` to
    `TrustStore { SystemApi(format!(..)) }`. Method doc records: pops the Windows
    root-store confirmation dialog; user-declined = error; **must run in an
    interactive session — never call from the daemon** (Phase 5 service is session 0).
  - `uninstall_system(fp)`: open "Root", enumerate, collect contexts whose
    `sha256(to_der()) == fp`, **finish iteration, then** `delete()` each (§0.3
    caveat). Zero matches = `Ok(())` (idempotent). Doc: deletion pops its own
    confirmation dialog.
  - `is_present_system(fp)`: enumerate + compare → `Ok(bool)`. Read-only, no dialog.
  - `is_trusted(_ca_path, fp)`: delegate to `is_present_system` with the Linux
    presence-is-trust comment (`os/linux.rs:282-287`).
  - `install_firefox_nss`/`uninstall_firefox_nss`: `Err(Unsupported { .. })` with a
    "Windows Firefox/NSS is a Phase 6 TODO" doc line; `browser_ca_trust` stays the
    trait default (§0.5).
  - `system_root_bundle()`: concat `store_root_pem` over
    `open_local_machine("Root")` + `open_current_user("Root")` (each open is
    best-effort — skip on error rather than failing the whole call, macOS-style
    `filter_map`); `Ok(None)` when no cert was rendered. Doc: read-only, no admin
    needed for LM reads; feeds `build_php_ca_bundle`.
- Trait doc touch-ups (`trust_store.rs:123-137`): the "always returns `NeedsHelper`
  in Phase 1" sentences gain "on Windows these perform the CurrentUser-Root operation
  directly (no elevation is required there)".
- **Tests** (`crates/yerd-platform/tests/windows_smoke.rs`) — same change:
  - Delete `trust_store_unsupported` (`:48-64`).
  - `trust_probe_reports_absent_for_random_fp`: `is_present_system(random_fingerprint(..))`
    → `Ok(false)` against the real Root store (read-only, dialog-free, CI-safe).
  - `memory_store_round_trip`: mint a test CA via `yerd_tls` (dev-dep already
    present), run `add_der`/`find_by_fp`/`delete` against `schannel::cert_store::Memory`
    → present → absent. Hermetic; no dialog; runs on the CI leg.
  - `install_rejects_fingerprint_mismatch`: `install_system(pem_of_ca_A, fp_of_ca_B)`
    → `TrustStore` error **before** any store is opened (assert no dialog risk by
    construction: the check precedes `open_current_user`).
  - `system_root_bundle_returns_public_roots`: `Ok(Some(pem))` containing at least
    one `BEGIN CERTIFICATE` on any real Windows host (CI runners have populated
    Root stores).
  - NSS methods still `Unsupported`.
- The **real Root-store round-trip is a manual DoD gate** on this machine (dialog),
  recorded in the PR: install → dialog → `certmgr.msc` shows it → probe true →
  `StatusReport.ca.trusted_system == Some(true)` → uninstall → dialog → probe false.

### Item 3 — CLI: `yerd elevate trust` Windows arm (`bin/yerd/src/elevate.rs`)

- Narrow the current stub's gate from `#[cfg(not(unix))]` to
  `#[cfg(not(any(unix, windows)))]` and add `#[cfg(windows)] mod windows_impl` with
  `pub async fn run_elevate(target, undo) -> ExitCode`:
  - **No root/admin check** (the whole point of CurrentUser Root); no
    `yerd-helper`, no `HelperInvocation`.
  - Target handling (pure helper, unit-tested): `Trust` → real work; `Resolver` →
    print "DNS setup arrives with the Windows elevation work (Phase 4)" and skip
    (exit 0); `Ports`/`Lan` → print "not needed on Windows — Yerd binds 80/443
    directly" and skip. `None` expands to the same trust→resolver→ports order with
    the two skips, mirroring the Unix `targets()` shape.
  - Trust flow: `transport::exchange(&Request::DaemonInfo)` (Windows pipe arm,
    Phase 1) → `ca_path` + `ca_fingerprint` → `CaFingerprint::from_hex` →
    `std::fs::read_to_string(ca_path)` → print a heads-up that **Windows will show a
    security-confirmation dialog** → `tokio::task::spawn_blocking` around
    `ActiveTrustStore.install_system(&pem, &fp)` (the add blocks on the dialog);
    `undo` → `uninstall_system(&fp)`. Success prints the trusted/untrusted line;
    failure prints the `PlatformError` and exits 1. Skip the Unix
    `report_browser_trust` follow-up (NSS is Unsupported on Windows, §0.5).
  - Note in the module doc why the daemon must never do this itself (session-0
    service in Phase 5 cannot show the dialog).
- `bin/yerd/src/cli.rs` doc copy (`:96`, `Elevate`/`Unelevate` help): note that on
  Windows no `sudo`/admin is needed and only `trust` is active until Phase 4.
- `bin/yerd/src/uninstall.rs` stays the non-Unix decline (full uninstall is Phase 4
  per the master plan — it removes the NRPT rule too, which doesn't exist yet).
- Tests: table test for the Windows target-expansion helper (pure). The end-to-end
  run is the Item 2 manual gate (dialog). No new e2e harness — the Phase 1 lifecycle
  test already proves the pipe transport.

### Item 4 — (folded into Item 2) PHP CA bundle on Windows

No separate code: once `system_root_bundle` is real, `startup.rs:138-144` +
`build_php_ca_bundle` write `{data}/cacert.pem`, `write_cli_ini`/FPM pick it up, and
doctor's `RebuildPhpCaBundle` auto-fix works. The `build_php_ca_bundle` unit tests
(`startup.rs:1297-1369`) are pure with injected roots and already run on the Windows
leg. Manual gate: from a site, `php -r "file_get_contents('https://<site>.test');"`
(or Laravel `Http::get`) verifies against the bundle — recorded in the PR.

### Item 5 — Site creation: Windows `build_job_bin` (`bin/yerdd/src/create_site/laravel.rs`)

- Replace the `#[cfg(not(unix))]` `Err(..)` arm (`:305-312`) with a real Windows
  implementation writing **job-local `.cmd` wrappers** into `{job_dir}/bin`:
  - `php.cmd` → `@"<php_cli>" %*` ; when `composer_phar` is `Some`:
    `composer.cmd` → `@"<php_cli>" "<phar>" %*`. CRLF endings, quoted absolute
    paths, no `@echo off` needed with the leading `@`. `cmd.exe` propagates the
    child's exit code as the batch exit code by default (single-command file).
  - Rendering goes through a small **pure helper**
    `fn cmd_wrapper(exe: &Path, first_arg: Option<&Path>) -> String` — table-tested
    on every OS (quoting, CRLF, arg forwarding) per the house "decisions in pure
    helpers" rule; the cfg'd arm only writes the file.
  - Update the function docs ("Unix-only" → per-OS) and the module doc note: nested
    `composer`/`php` invocations inside `laravel new` go through PHP `proc_open` →
    `cmd.exe`, which resolves `.cmd` via `PATHEXT` (unlike Rust's `Command::new` —
    which is why the *outer* spawn stays `php.exe <installer.phar>`, `:108-123`).
  - This is deliberately **job-local** and independent of Phase 5's `{data}/bin`
    `.cmd` shim delivery (`tools/mod.rs:296-301` no-op untouched).
- `sh_quote`/Unix arm byte-identical. `composed_path` already OS-correct
  (`join_paths`).
- Tests: unit tests beside the existing Unix ones (`laravel.rs:501-533`) —
  `#[cfg(windows)]` `build_job_bin` writes both wrappers with the phar, only
  `php.cmd` without; the pure `cmd_wrapper` table runs everywhere.
- Manual smoke (with Item 7): GUI "New site" → Laravel scaffold on this machine;
  WordPress flow (no cfg-gated code, §0.8) exercised the same way. If nested
  `composer` resolution misbehaves under a specific installer version, the recorded
  fallback is invoking composer as `php.exe <phar>` via `COMPOSER_BINARY`-style env —
  investigate then, not speculatively.

### Item 6 — SCRIPT_FILENAME separator pin (`crates/yerd-proxy/src/pure/cgi_params.rs`)

- Add table cases to the existing pure test module:
  - `#[cfg(windows)]` case: `document_root = C:\sites\shop`, `script_rel =
    wp-admin\index.php` (what `resolve_script`'s `Path::join` yields on Windows) →
    assert `SCRIPT_FILENAME == C:\sites\shop\wp-admin\index.php` (native, PHP-legal),
    `DOCUMENT_ROOT` native, and **`SCRIPT_NAME == "/wp-admin/index.php"` with no
    backslash** (pins the `:83` normalization — the classic bug).
  - Cross-OS case: a `script_rel` built with `Path::new("wp-admin").join("index.php")`
    asserting `SCRIPT_NAME` contains only `/` on every OS.
- No production code change expected; if the assertions expose one, it's a
  `cgi_params` fix, still pure.
- The four `#[cfg(unix)]` symlink integration tests stay Unix-only with a one-line
  comment (Windows symlink creation needs Developer Mode; `storage:link`-on-Windows
  validation is a recorded post-MVP TODO).

### Item 7 — GUI smoke (+ optional in-app trust arm)

- **Required (manual, this machine):** run the Tauri dev app against the dev daemon;
  verify sites list, create-site (needs Item 5), and PHP-version-per-site; record
  results + any Unix-isms found in the PR. Expected copy quirks (fix only if
  trivially small, else list as Phase 6 GUI-polish input): `EnvironmentCard`'s
  "use `yerd elevate` in a terminal" line is now accurate on Windows (post-Item 3);
  doctor chips render the Item 1 Windows remedies automatically (they come over IPC).
- **Optional / stretch (not required for DoD; flag before doing):** mirror
  `mac_trust.rs` for Windows — `#[cfg(windows)]` arms in `commands.rs`
  `trust_ca`/`untrust_ca` that fetch `DaemonInfo` facts, re-verify the fingerprint,
  and call `yerd_platform::ActiveTrustStore` `install_system`/`uninstall_system` in
  `spawn_blocking` (the GUI process is interactive, so the dialog shows correctly);
  loosen the `platform === "macos"` gates in `EnvironmentCard.vue:200,257` and the
  `client.ts` doc comments. `untrust_ca`'s "system-wide trust remains" return is
  always `false` on Windows (there is no second store we manage). Deferring keeps
  Phase 3 minimal — the CLI path is the DoD surface.

### Item 8 — Docs, instruction files, stale comments

- `.github/instructions/yerd-platform.instructions.md:20-22`: the "Windows has a
  real `Paths` impl and aliases every other trait" sentence is already stale
  (Phase 2 added `PortBinder`) — rewrite as "Windows implements a growing subset
  (`Paths`, `PortBinder`, `PortRedirector`, `TrustStore`); the remainder alias the
  `unsupported` stub — keep that stub total."
- `.github/copilot-instructions.md:107-110`: same parenthetical, same fix.
- `os/windows.rs` module doc (`:1-7`) and `os/mod.rs` doc (`:1-7`): list the real
  impls; drop "Only `WindowsPaths` is real".
- `trust_store.rs` trait docs (Item 2), `elevate.rs` module doc (Item 3),
  `laravel.rs` docs (Item 5) — land with their items.
- `TODO.md`/PR description: record the two tracked TODOs this phase creates —
  Windows Firefox/NSS trust (Phase 6, locked) and `storage:link` symlink serving
  validation on Windows.

---

## Ordering & compile checkpoints

```
Item 1 (PortRedirector + doctor copy) ── independent; no new dep; can land first
Item 2 (WindowsTrustStore, schannel)  ── needs ESCALATION ack (new pinned dep);
                                         alias flip + windows_smoke rewrite in ONE change
Item 3 (CLI elevate trust arm)        ── needs 2 (calls the real trait impl)
Item 4 (PHP CA bundle)                ── folded into 2 + manual gate
Item 5 (create_site .cmd job bin)     ── independent (can parallel 1)
Item 6 (cgi_params separator test)    ── independent
Item 7 (GUI smoke [+optional trust])  ── needs 2, 3, 5
Item 8 (docs)                         ── with or after the item each line documents
```

Every item independently passes the full gate on ubuntu / ubuntu-arm / macos-14 /
windows-latest.

## IPC-contract / pure-crate / boundary flags (surfaced, not worked around)

1. **No IPC wire change anywhere in Phase 3.** All `StatusReport` fields
   (`ca.trusted_system`, `foreign_web_listener`, `web_unbound`, `browser_trust`) and
   all `DiagnosisCode`s pre-exist; Windows merely starts populating them.
2. **No `HelperInvocation`/`yerd-helper` change.** Windows trust is unprivileged and
   deliberately bypasses the helper; the argv contract and helper binary are
   untouched (Phase 4 will extend them for NRPT).
3. **Trait-contract semantic shift, documented not hidden:** `install_system`/
   `uninstall_system` return `Ok` after doing real work on Windows, vs `NeedsHelper`
   on Unix. No existing caller breaks (the daemon never calls them; the Unix CLI
   goes through the helper; the only Windows caller is new Item-3/7 code). Trait docs
   updated in Item 2.
4. **`schannel` enters `yerd-platform`'s Windows-target graph** — the ESCALATION ack.
   Not on any `no_runtime_deps` forbidden list (`native-tls` forbidden ≠ `schannel`;
   dependency direction means no TLS crate is pulled); rustls-only rule intact.
5. **Doctor stays pure**: Windows messaging uses compile-time `cfg!` string selection
   (existing precedent at `lib.rs:430-436`), no I/O added to the crate.
6. **Interactive-session constraint** (new invariant worth a doc line in Item 2):
   CurrentUser-Root mutations must run in CLI/GUI processes, never `yerdd` — this is
   load-bearing for Phase 5's session-0 service and is why no daemon-side trust
   op is added.

## Deviations from WINDOWS_PLAN.md (flag for confirmation)

1. **`PortBinder` work is already done** (Phase 2 pull-forward, recorded there);
   Phase 3's port scope reduces to the `WindowsPortRedirector` + doctor copy (Item 1).
2. **No active `urlacl`/`netsh` enumeration in doctor** — the passive listener probe
   plus messaging that names `netsh http show servicestate`/W3SVC/Skype is the
   minimal check; active reservation parsing adds a subprocess for little signal.
3. **`system_root_bundle` Windows impl is added** (not in the master plan's Phase 3
   text) — it is the missing link for the PHP CA bundle (`php_trusts_ca`,
   `yerd doctor fix`) and costs ~30 lines once schannel is present (§0.5, Item 2/4).
4. **`create_site`'s "existing `#[cfg(windows)]` branches" turned out to be a hard
   `Err` stub** (`build_job_bin`, §0.8) — Phase 3 replaces it with job-local `.cmd`
   wrappers (Item 5), foreshadowing but not touching Phase 5's shim delivery.
5. **CurrentUser-Root confirmation dialogs (add *and* delete) make the real-store
   round-trip untestable in CI** — covered instead by a hermetic `Memory`-store test
   + read-only real-store probes in CI, and a recorded manual gate on this machine.
6. **GUI in-app trust** (macOS parity) is scoped as optional (Item 7) — the master
   plan's Phase 3 GUI scope is the sites flows; the CLI is the trust surface.

## New / changed dependencies

| Dep | Where | Why | Notes |
|---|---|---|---|
| `schannel = "0.1"` (0.1.29 today) | workspace table + `yerd-platform` `[target.'cfg(windows)']` | safe CurrentUser/LocalMachine cert-store API (install/probe/uninstall CA, host-roots enumeration) | ESCALATION Option A; sole dep `windows-sys 0.61` already in `Cargo.lock`; MIT; MSRV 1.71; cert-store modules only — **never** its TLS; pin per house style |

Nothing else. No `unsafe` anywhere; no forbid-lift; no new crates for Items 1, 3, 5, 6.

## Definition of done (checkable)

- [ ] `cargo fmt/clippy -D warnings/test --workspace` green on ubuntu-22.04,
      ubuntu-22.04-arm, macos-14, windows-latest.
- [ ] Windows CI leg runs (not cfg'd out): the rewritten `windows_smoke.rs` trust
      tests (real-store read-only probe, `Memory` round-trip, fingerprint-mismatch
      reject, `system_root_bundle` non-empty), the `WindowsPortRedirector` smoke,
      the doctor Windows-copy assertions, the `cgi_params` separator pins, and the
      `build_job_bin`/`cmd_wrapper` tests.
- [ ] On this machine (recorded in the PR): `yerd elevate trust` → Windows
      confirmation dialog → CA visible in `certmgr.msc` (CurrentUser Root) →
      `yerd status`/GUI show trusted; `yerd elevate trust --undo` (or `unelevate`)
      → delete dialog → probe false. No UAC prompt at any point.
- [ ] `yerd site` create (GUI New-site flow) succeeds on Windows; browsing
      `https://<site>.test` with a manual hosts entry (or `curl --resolve`) shows a
      valid chain to the user-store CA in Edge/Chrome — green padlock, PHP page
      renders via FastCGI-over-TCP; HTTP redirects to HTTPS; a second site on a
      different PHP version serves under its own SNI cert.
- [ ] `{data}/cacert.pem` exists on Windows (host roots + Yerd CA); PHP outbound
      HTTPS to a `.test` site verifies (no cURL error 60);
      `StatusReport.ca.php_trusts_ca == Some(true)`.
- [ ] `yerd doctor` on Windows: with IIS (or any listener) squatting port 80, the
      `ForeignWebListener` finding appears with the Windows remedy (no `sudo`, no
      `elevate ports`); with ports free, no port findings.
- [ ] macOS/Linux behaviour byte-identical (doctor Unix strings unchanged; trait
      Unix impls untouched; `unsupported` stub still total; helper argv contract
      test unchanged).
- [ ] Instruction-file staleness fixed (`yerd-platform.instructions.md`,
      `copilot-instructions.md` Windows parentheticals) and the two new TODOs
      (Firefox/NSS Phase 6, symlink-serving validation) recorded.
