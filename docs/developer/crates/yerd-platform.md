# yerd-platform

`yerd-platform` is Yerd's **OS abstraction layer**. It is the single place where the rest of the workspace touches platform-specific behaviour - trust stores, DNS resolver redirection, privileged-port binding, filesystem layout, and process metrics. Everything above it (the daemon, the CLI, `yerd-tls`, `yerd-dns`, `yerd-doctor`) consumes the traits defined here and never reaches for `#[cfg(target_os = ...)]` itself.

The crate has three jobs and keeps them strictly separated:

1. **Define the traits** - one trait per platform concern, with typed error and result types.
2. **Implement them once per OS**, selected at compile time, exposed through `Active*` aliases so callers name the concrete impl without a `cfg`.
3. **Keep all decision logic pure** in the `pure` module so it is unit-tested in-memory with no I/O, clock, environment, or async runtime.

A fourth, cross-cutting responsibility is the **privilege boundary**: this crate is unprivileged library code and never spawns the helper. See [Cross-Platform Model](../cross-platform) and [Elevation & Privileges](../../guide/elevation) for the wider picture.

::: info Source
[`crates/yerd-platform`](https://github.com/forjedio/yerd/tree/main/crates/yerd-platform). The crate is `#![forbid(unsafe_code)]`.
:::

## Module map

```text
src/
  lib.rs            re-exports the traits, types, and Active* aliases
  detect.rs         ProjectSignalSource trait + gather_project_signals (web-root signals)
  error.rs          PlatformError + typed reason enums + ops:: tag constants
  helper.rs         HelperInvocation (typed) + to_argv / from_argv wire contract
  metrics.rs        SystemMetrics trait (Option-returning, best-effort)
  paths.rs          Paths trait + PlatformDirs struct
  port_binder.rs    PortBinder trait + BoundPort / PortPair
  port_redirect.rs  PortRedirector trait + loopback_port_reachable / loopback_redirect_reaches_proxy
  resolver.rs       ResolverInstaller trait
  trust_store.rs    TrustStore trait + CaFingerprint + NssOutcome / NssFailure
  os/
    mod.rs          cfg-selects exactly one impl; exposes `active` aliases
    linux.rs        Linux* impls
    macos.rs        Macos* impls (+ security-framework)
    unsupported.rs  Unsupported* stubs (Windows, Phase 1)
  pure/
    mod.rs
    firefox.rs          parse_profiles_ini
    pem_match.rs        find_by_fingerprint, sha256, der_to_pem
    pf_anchor.rs        compose/insert/remove macOS pf redirect rules
    port_plan.rs        classify_desired / classify_fallback
    proc_metrics.rs     parse_vmrss_bytes / parse_loadavg (Linux /proc)
    ps_metrics.rs       parse_ps_rss_bytes (macOS `ps`)
    resolv_conf.rs      select supported Linux resolver backend
    networkmanager_dnsmasq.rs compose/match NetworkManager snippets
    resolved_drop_in.rs compose/parse systemd-resolved drop-in
    resolver_file.rs    compose/parse macOS /etc/resolver/<tld>
```

Runtime dependencies are deliberately tiny: `thiserror`, `directories`, `sha2`, `hex`, `pem`, `serde_json` (only to read `composer.json` during signal gathering), plus `yerd-tls` and `yerd-core` (for the `ProjectSignals` / `Detection` types the detector feeds), with `security-framework` gated to macOS only. A test (`tests/no_runtime_deps.rs`) walks the resolved dependency graph (scoped with `--filter-platform` so cfg gates apply) and asserts that `tokio`, `anyhow`, `reqwest`, `openssl`, `openssl-sys`, and `native-tls` never appear in the normal-kind runtime graph - none of which `serde_json` or `yerd-core` pull in.

## The core traits

Each trait is intentionally narrow. With one exception, fallible methods return `Result<_, PlatformError>`.

### `Paths`

```rust
pub trait Paths {
    fn resolve(&self) -> Result<PlatformDirs, PlatformError>;
}
```

`resolve` returns a `PlatformDirs` with five directories. **Existence is not guaranteed** - callers are responsible for `create_dir_all` before writing.

| Field     | Meaning                          | Linux                                                       | macOS                              |
| --------- | -------------------------------- | ----------------------------------------------------------- | ---------------------------------- |
| `config`  | User config                      | `~/.config/yerd`                                            | macOS config dir                   |
| `data`    | Persistent data (CA + leaf certs)| XDG data home                                               | macOS data dir                     |
| `state`   | Long-lived state                 | `XDG_STATE_HOME` (distinct from `data`)                     | collapses to `data`                |
| `cache`   | Logs, downloads                  | XDG cache home                                              | macOS cache dir                    |
| `runtime` | IPC socket + PID file            | `XDG_RUNTIME_DIR/yerd`, else `/tmp/yerd-$UID`               | `/tmp/yerd-$UID`                   |

The Linux impl uses the `directories` crate, falling back to reading the real UID from `/proc/self/status` when `XDG_RUNTIME_DIR` is unset. macOS uses `ProjectDirs::from("io", "yerd", "Yerd")` and deliberately chooses a **deterministic, uid-derived** `/tmp/yerd-$UID` for `runtime` rather than `std::env::temp_dir()` (the per-session `/var/folders/…` path). Determinism is load-bearing: `yerd elevate`, running as root under `osascript`/`sudo`, must reconstruct the socket path from `SUDO_UID` alone without privileged FFI.

::: warning runtime is security-sensitive
When the `/tmp/yerd-$UID` fallback is used, the caller must `mkdir(mode=0o700)` and, if the directory already exists, verify ownership (`uid == geteuid()`) and mode (`0o700`) before using it. `yerd-platform` deliberately does not do this - it only computes the path; the daemon's `secure_fs` owns the fail-closed creation.
:::

### `TrustStore`

```rust
pub trait TrustStore {
    fn install_system(&self, ca_pem: &str, fp: &CaFingerprint) -> Result<(), PlatformError>;
    fn uninstall_system(&self, fp: &CaFingerprint) -> Result<(), PlatformError>;
    fn is_present_system(&self, fp: &CaFingerprint) -> Result<bool, PlatformError>;
    // Effectively *trusted* for SSL, not merely present. Defaulted to
    // `Unsupported` (the only defaulted method); macOS uses `security
    // verify-cert`, Linux delegates to `is_present_system`.
    fn is_trusted(&self, ca_path: &Path, fp: &CaFingerprint) -> Result<bool, PlatformError>;
    fn install_firefox_nss(&self, ca_pem: &str) -> Result<NssOutcome, PlatformError>;
}
```

The CA is identified by a `CaFingerprint` - a newtype around `[u8; 32]` (SHA-256 over the cert's DER body) with a private field, so callers cannot construct an unchecked fingerprint. Its canonical wire form is **64 lowercase hex characters**; `from_hex` strictly rejects uppercase, wrong length, and non-hex so the form stays byte-stable across the helper argv boundary.

- `install_system` / `uninstall_system` always return `Err(PlatformError::NeedsHelper { .. })` in Phase 1. They are write operations against a root-owned store, so the daemon materialises the matching `HelperInvocation` and runs `yerd-helper`.
- `is_present_system` is a **read-only, unprivileged presence probe**. It reports whether a CA matching the fingerprint is *in* the store - not whether it is trusted for SSL by every consumer. On macOS it enumerates `/Library/Keychains/System.keychain` via `security-framework` and hashes each cert's DER; on Linux it iterates the distro's anchor directory and hashes each PEM block (the candidate directories are `/usr/local/share/ca-certificates`, `/etc/pki/ca-trust/source/anchors`, and `/etc/ca-certificates/trust-source/anchors`).
- `install_firefox_nss` is the one trust operation that runs **per-user and unprivileged** - Firefox keeps its own NSS database. It is best-effort and returns `Ok(NssOutcome)` even on partial failure:

```rust
pub struct NssOutcome {
    pub profiles_attempted: usize,
    pub profiles_succeeded: usize,
    pub failures: Vec<(PathBuf, NssFailure)>, // per-profile, in attempt order
    pub certutil_missing: bool,
}

pub enum NssFailure { CertutilMissing, CertutilExit(i32), DbMissing }
```

The caller decides whether to surface the degraded outcome. See [HTTPS & Certificates](../../guide/https) for the user-facing story.

### `ResolverInstaller`

```rust
pub trait ResolverInstaller {
    fn install(&self, tld: &str, addr: SocketAddr) -> Result<(), PlatformError>;
    fn uninstall(&self, tld: &str) -> Result<(), PlatformError>;
    fn is_installed(&self, tld: &str, addr: SocketAddr) -> Result<bool, PlatformError>;
}
```

`addr` is the IP+port the OS resolver should forward `.test` lookups to. The Phase-1 daemon always passes `127.0.0.1:<port>`, but the trait takes a full `SocketAddr` so a future version can move the DNS responder without a breaking change. `install`/`uninstall` return `NeedsHelper`; `is_installed` reads public config and is unprivileged. Both `uninstall` and `is_installed` are idempotent for an absent TLD (`Ok(())` / `Ok(false)`).

`is_installed` must verify the on-disk config points at **this** `addr` - a stale file aimed elsewhere (e.g. a Valet/Herd leftover on `:53`) must report `false` so the redirect gets re-installed. See [DNS & .test Domains](../../guide/dns).

### `PortBinder`

```rust
pub trait PortBinder {
    fn bind(&self, port: u16) -> Result<BoundPort, PlatformError>;
    fn bind_pair(&self, desired: (u16, u16), fallback: (u16, u16))
        -> Result<PortPair, PlatformError>;
}
```

`BoundPort` wraps a plain `std::net::TcpListener` (not the tokio variant) so `yerd-platform` keeps `tokio` out of its public surface; `yerd-proxy` converts via `tokio::net::TcpListener::from_std`. `BoundPort::port()` reads the resolved port from `local_addr()`, so it is correct even when binding `0`.

`bind_pair` binds HTTP+HTTPS atomically with a fallback. It attempts the desired pair, then:

- both succeed → keep them;
- one fails with a retry-trigger kind (`PermissionDenied`, `AddrInUse`, `AddrNotAvailable`) → drop any partial listener and retry the fallback pair;
- one fails with any other kind → surface `PlatformError::Bind` immediately *without* trying the fallback.

If both pairs fail, the error is `PlatformError::BindPair` carrying **all four** `io::ErrorKind`s, so the daemon can tell "setcap missing" (`PermissionDenied` everywhere) from "port already in use" (`AddrInUse` on the desired pair) and message the user accordingly. The classification itself is pure - see [`port_plan`](#port-plan).

### `PortRedirector`

```rust
pub trait PortRedirector {
    fn is_active(&self) -> Option<bool>;
    fn foreign_web_listener(&self) -> Option<bool>; // default impl, cross-platform
}
```

On macOS the unprivileged daemon cannot bind 80/443, so `yerd elevate ports` installs a pf `rdr` redirect to its rootless ports. Because the daemon still binds the high ports, `StatusReport.http.fell_back` stays `true` even when the redirect works - the doctor needs an independent signal that 80/443 are *actually reachable*. `is_active` is therefore an **active, unprivileged** check, and it goes further than "something answers": it speaks HTTP to loopback and requires the proxy's `Server` marker (`yerd_core::PROXY_SERVER_ID`) on the reply, so a foreign listener or a stale `pf` rule can't read as a live Yerd redirect.

```rust
// Bare reachability: does *anything* answer?
pub fn loopback_port_reachable(port: u16) -> bool { /* TCP connect, 250ms */ }

// Identity-confirming: is the answer *this* daemon's proxy? (Server: yerd marker)
pub fn loopback_redirect_reaches_proxy(port: u16) -> bool { /* HTTP probe */ }
```

`is_active` returns `None` on Linux (it binds the privileged ports directly after `setcap`). `foreign_web_listener` is the inverse, useful signal and is **cross-platform**: a default trait method that returns `Some(true)` when a privileged web port answers but the Yerd proxy marker is absent (a non-Yerd process squatting 80/443), `Some(false)` otherwise. The daemon surfaces it as `StatusReport.foreign_web_listener`, and `yerd-doctor` raises `ForeignWebListener` from it. The `unsupported` stub overrides it back to `None`.

### `SystemMetrics`

```rust
pub trait SystemMetrics {
    fn rss_bytes(&self, pid: u32) -> Option<u64>;
    fn load_average(&self) -> Option<[f64; 3]>;
}
```

Unlike every other trait here, metrics return `Option`, not `Result`. `None` collapses two cases - "OS unsupported" and "transient read failed" - because the only caller (`yerd status`) treats both identically: show nothing. The OS impls do only the file read / subprocess; the actual decoding lives in the table-tested `proc_metrics` / `ps_metrics` parsers.

## OS implementations and the `Active*` aliases

`os/mod.rs` compiles exactly one of `linux`, `macos`, or `unsupported` per build and re-exports it under uniform aliases:

```rust
pub(crate) mod active {
    #[cfg(target_os = "linux")]
    pub use super::linux::{
        LinuxPaths as ActivePaths, LinuxPortBinder as ActivePortBinder,
        LinuxPortRedirector as ActivePortRedirector,
        LinuxResolverInstaller as ActiveResolverInstaller,
        LinuxSystemMetrics as ActiveSystemMetrics, LinuxTrustStore as ActiveTrustStore,
    };
    // macos / unsupported arms are symmetric
}
```

`lib.rs` re-exports these, so callers write `ActiveTrustStore`, `ActivePaths`, etc. and the right concrete type is selected at compile time - no `cfg` leaks into consumer crates.

The **`unsupported` stub** (Windows in Phase 1) implements every trait so `cargo check --workspace` stays green on any host. Every fallible method returns `Err(PlatformError::Unsupported { operation })`; `SystemMetrics` returns `None`; `PortRedirector` returns `None`. `tests/unsupported.rs` (gated to non-Linux/non-macOS targets) asserts each method returns `Unsupported`.

## The `pure` module

Every function in `pure` is sync, runtime-free, and free of I/O, clock reads, and environment lookups. The OS impls do the reads/writes and call into `pure` for the decision; `pure` is table-tested in isolation.

### firefox

`parse_profiles_ini(text) -> Vec<Profile>` parses Firefox `profiles.ini`. It returns each `[Profile<N>]` section's `Name`, `Path`, `IsRelative`, and `Default`, silently ignoring `[General]`/`[Install…]` sections, comments, and profiles with no `Path`. Output `path` is raw - the caller joins relative paths against the `profiles.ini` parent, which `pure` does not know about.

### pem_match

`find_by_fingerprint(blobs: &[(PathBuf, Vec<u8>)], fp: &[u8;32]) -> Result<Option<PemMatch>, PathBuf>` searches pre-read PEM blobs (the caller does the I/O) for the first `CERTIFICATE` block whose DER body hashes to `fp`, returning the matching path and 0-based block index. `Err(path)` signals a blob that failed PEM parsing, which the OS layer translates to `TrustStoreErrorReason::AnchorPemInvalid`. A blob with no certificate blocks (e.g. a README in an anchor dir) is `Ok(None)`, not an error. Also exports `sha256`, `der_to_pem`, and `fingerprint_of_first_cert_in_pem`.

### pf_anchor

Composes the macOS pf redirect. Key constants: `ANCHOR_NAME = "dev.yerd"`, `ANCHOR_PATH = "/etc/pf.anchors/dev.yerd"`, `PLIST_PATH = "/Library/LaunchDaemons/dev.yerd.pf.plist"`, `PF_CONF_PATH = "/etc/pf.conf"`.

```text
rdr pass on lo0 inet proto tcp from any to any port 80  -> 127.0.0.1 port <http_to>
rdr pass on lo0 inet proto tcp from any to any port 443 -> 127.0.0.1 port <https_to>
```

`on lo0` is load-bearing: pf `rdr` only fires for `127.0.0.1→127.0.0.1` traffic when the rule is anchored to `lo0`. Functions: `compose_anchor_rules`, `is_installed`, `insert_anchor_refs`, `remove_anchor_refs`, `compose_launchdaemon_plist`. Inserted lines carry a `# yerd-managed` marker so insertion is idempotent and removal is unambiguous; the `rdr-anchor` reference is placed after the last existing translation anchor (pf requires the translation section before filter rules). The design **edits** `/etc/pf.conf` rather than loading a self-contained ruleset, because `pfctl -f <full-ruleset>` would flush Apple's default `com.apple/*` anchors. The boot-persistence plist is one-shot (`RunAtLoad`, no `KeepAlive`).

### port_plan

The pure classifier behind `PortBinder::bind_pair`:

```rust
pub fn classify_desired(http: BindOutcome, https: BindOutcome) -> DesiredPairAction; // KeepDesired | UseFallback | HardFail(ErrorKind)
pub fn classify_fallback(http: BindOutcome, https: BindOutcome) -> FallbackPairAction; // KeepFallback | BothFailed
pub fn is_retry_kind(kind: ErrorKind) -> bool; // PermissionDenied | AddrInUse | AddrNotAvailable
```

The precedence is pinned by tests: a hard (non-retry) kind always beats a retry kind, and on the desired pair, `http` is inspected before `https`.

### resolv_conf, networkmanager_dnsmasq, resolved_drop_in, resolver_file (DNS)

- `resolv_conf` prefers systemd-resolved, selects NetworkManager only from its positive `/etc/resolv.conf` generator marker, and otherwise returns unsupported. It also validates NetworkManager's post-reload `127.0.0.1` dnsmasq nameserver.
- `networkmanager_dnsmasq` composes and strictly matches the `[main] dns=dnsmasq` override and per-TLD `server=/test/127.0.0.1#1053` rule.
- `resolved_drop_in::compose(tld, addr)` emits the Linux drop-in `[Resolve]\nDNS=<addr>\nDomains=~<tld>\n` (`/etc/systemd/resolved.conf.d/yerd-<tld>.conf`); `parse` / `matches` tolerate comments and extra keys so the `is_installed` probe is robust against operator edits.
- `resolver_file::compose(addr)` emits the macOS `/etc/resolver/<tld>` body `nameserver <ip>\nport <port>\n`; `parse` / `matches` ignore comments and ordering, defaulting a missing `port` to `53` per `resolver(5)`. `restorable(text)` (= `parse(text).is_some()`) is the pure guard the helper uses before writing a backup back over `/etc/resolver/<tld>` on `unelevate resolver`, so an empty/garbage backup is never restored. `backup_filename` / `parse_backup_secs` / `latest_backup` are the path-logic for the timestamped backups (`<tld>-<unixsecs>.conf`); the macOS helper restores the newest and clears the rest, the daemon reads the newest to report `ResolverBackupSaved`.

### proc_metrics, ps_metrics

- `proc_metrics::parse_vmrss_bytes(status)` reads the `VmRSS:` line of `/proc/<pid>/status` (kibibytes → bytes); `parse_loadavg(text)` reads the first three figures of `/proc/loadavg`. Using `VmRSS` avoids needing `_SC_PAGESIZE`, keeping the parser `unsafe`-free.
- `ps_metrics::parse_ps_rss_bytes(stdout)` parses the headerless first token of `ps -o rss= -p <pid>` (kibibytes → bytes) on macOS, which has no cheap `unsafe`-free per-process RSS source in `std`. All three return `None` on malformed input - metrics are best-effort.

## Web-root signal gathering (`detect`)

`detect.rs` is the **I/O half of web-root detection** - the *decision* half is the pure [`yerd_core::detect`](./yerd-core#detect). Gathering reads a project directory and produces an in-memory `yerd_core::ProjectSignals`; the daemon then calls `yerd_core::detect` on it to pick the served subdirectory.

Per the crate's "side effects behind traits" rule, gathering is exposed through a trait so callers can inject a fake:

```rust
pub trait ProjectSignalSource {
    fn gather(&self, project_root: &Path) -> ProjectSignals;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct FsSignalSource;            // the real, filesystem-backed impl

pub fn gather_project_signals(project_root: &Path) -> ProjectSignals;
```

`gather_project_signals` is **best-effort and infallible** - a missing or malformed `composer.json`, an unreadable directory, etc. simply contribute fewer signals. It reads only the project root and the immediate candidate web dirs (`yerd_core::detect::WEB_DIR_CANDIDATES`), never recursively: it parses `composer.json`'s `require`/`require-dev` package names (lowercased), stats the `ROOT_MARKERS` (file *or* dir presence), and checks each candidate dir for an `index.php` front controller. Tempdir fixture tests (Laravel-like, WordPress-like, plain, empty, malformed-JSON) pin the gather→detect pipeline end to end.

This is the only place in the crate that touches a non-config filesystem path, and it is not OS-gated - the conventions are the same on every platform.

## The privilege boundary

This is the crate's most important invariant. `yerd-platform` is **unprivileged**. Any operation that needs root does not perform it - it returns `PlatformError::NeedsHelper { operation }`, where `operation` is one of the `&'static str` tags in `error::ops` (the single source of truth for these strings, also used as the leading argv element).

```rust
pub enum PlatformError {
    NeedsHelper { operation: &'static str },
    Unsupported { operation: &'static str },
    MissingHomeDir,
    TrustStore { reason: TrustStoreErrorReason },
    Resolver { reason: ResolverErrorReason },
    Bind { port: u16, source: std::io::Error },
    BindPair { reason: BindPairErrorReason },
    Io { path: PathBuf, source: std::io::Error },
    MissingTool { tool: &'static str, install_hint: Option<&'static str> },
}
```

`PlatformError` is `#[non_exhaustive]`, as are its reason sub-enums, so adding variants is semver-compatible. It is intentionally **not** `Clone + Eq` because two variants wrap `std::io::Error`.

When the daemon sees `NeedsHelper`, it builds a typed `HelperInvocation` and hands it to its subprocess spawner. Values stay typed all the way until `to_argv` serialises them at the spawn site - there is no `Vec<String>` round-trip in between.

```rust
pub enum HelperInvocation {
    InstallCa { ca_pem_path: PathBuf, fp: CaFingerprint },
    UninstallCa { fp: CaFingerprint },
    InstallResolver { tld: String, addr: SocketAddr },
    UninstallResolver { tld: String },
    Setcap { daemon_binary: PathBuf },
    InstallPortRedirect { http_from: u16, http_to: u16, https_from: u16, https_to: u16 },
    UninstallPortRedirect,
}
```

::: warning This crate never spawns the helper
The OS impls never call `Command::new(...)` for a privileged operation. A privileged caller owns the spawn: the daemon for its own setup, or `yerd elevate` running under `sudo`. `yerd-platform` only computes *what* should happen and serialises the request. See [yerd-helper](../binaries/yerd-helper) and [Elevation & Privileges](../../guide/elevation).
:::

`HelperInvocation::to_argv` / `from_argv` are a **wire contract** with the `yerd-helper` binary, pinned by `tests/helper_argv_shape.rs` (frozen golden vectors) and round-trip tested in the unit suite. The first element is always the op tag; subsequent elements alternate `--flag` and a single typed value:

```sh
install-ca --pem /run/user/1000/yerd/ca.pem --fingerprint <64-hex>
uninstall-ca --fingerprint <64-hex>
install-resolver --tld test --addr 127.0.0.1:5353
uninstall-resolver --tld test
setcap --binary /usr/bin/yerdd
install-port-redirect --http-from 80 --http-to 8080 --https-from 443 --https-to 8443
uninstall-port-redirect
```

`from_argv` is strict: unknown ops, unknown/missing flags, missing values, bad fingerprints (rejects uppercase/short), bad socket addresses, bad ports, non-UTF-8 values, and trailing argv each map to a typed `ArgvParseError`. Adding a field, reordering, or renaming a flag trips the golden test - which is exactly the point.

## Design invariants summary

| Invariant | Enforced by |
| --- | --- |
| No `tokio`/`anyhow`/`reqwest`/OpenSSL in the runtime graph | `tests/no_runtime_deps.rs` |
| Helper argv shape is frozen | `tests/helper_argv_shape.rs` + round-trip unit tests |
| Fingerprint wire form is 64 lowercase hex | `CaFingerprint::from_hex` + tests |
| Every trait method on unsupported OSes returns `Unsupported` | `tests/unsupported.rs` |
| `bind_pair` fallback/precedence rules | `pure::port_plan` tests |
| All decision logic is pure and I/O-free | `pure` module + per-submodule tests |
| `#![forbid(unsafe_code)]` | compiler |

## Related pages

- [Cross-Platform Model](../cross-platform) - how the cfg-gated layout and helper boundary fit together.
- [Crates Overview](../crates) - where `yerd-platform` sits in the workspace.
- [yerd-tls](./yerd-tls) and [yerd-dns](./yerd-dns) - primary consumers of `TrustStore` and `ResolverInstaller`.
- [yerd-helper (privileged)](../binaries/yerd-helper) - the binary that executes a `HelperInvocation`.
- [Elevation & Privileges](../../guide/elevation) - the user-facing view of the privilege boundary.
