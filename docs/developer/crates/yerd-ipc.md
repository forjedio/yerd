# yerd-ipc

The IPC protocol, framing, and codec that sit between `yerdd` (the daemon) and its clients - the [`yerd` CLI](../binaries/yerd) and the [Tauri GUI](../gui). The crate owns the *shape of the wire*: the message envelopes, the length-prefixed frame codec, the error types, and an optional async transport. It owns nothing about *where* the wire is (Unix sockets, named pipes); that binding lives in the binaries.

::: tip Looking for the protocol semantics?
This page documents the **crate** - its modules, public API, and design decisions. For the end-to-end protocol walkthrough (connection lifecycle, request/response choreography, version-skew handling) see [IPC Protocol](../ipc-protocol).
:::

## At a glance

| Property | Value |
| --- | --- |
| Crate path | [`crates/yerd-ipc`](https://github.com/forjedio/yerd/tree/main/crates/yerd-ipc) |
| Default build | **pure**: no sockets, no async, no I/O |
| Runtime deps | `yerd-core`, `serde`, `serde_json`, `thiserror` |
| Optional dep | `tokio` (only via the `transport` feature) |
| Wire format | 4-byte big-endian `u32` length prefix + UTF-8 JSON payload |
| Default frame cap | 16 MiB (`DEFAULT_MAX_FRAME`) |
| Protocol version | `PROTOCOL_VERSION = 1` (reserved; no handshake yet) |

## Module map

The crate is small and deliberately flat. Every module is private; the public surface is the curated re-export list in `lib.rs`.

```
src/
├── lib.rs         re-exports, PROTOCOL_VERSION, types module
├── frame.rs       encode_frame + FrameDecoder (pure length-prefix codec)
├── message.rs     encode_message / decode_message (serde_json wrappers)
├── request.rs     Request enum (client → daemon)
├── response.rs    Response enum + ErrorCode + PhpUpdate (daemon → client)
├── create.rs      CreateSiteSpec + Framework/StarterKit/Database/… (the `yerd create` site-scaffold spec) + JobId/JobState
├── dump.rs        DumpCategory / DumpEvent / DumpCounts / DumpExtStatus (Laravel ▸ Dumps telemetry)
├── status.rs      StatusReport / Diagnosis / FixReport and friends
├── error.rs       FrameError, IpcError, IpcErrorKind
└── transport.rs   #[cfg(feature = "transport")] async read/write helpers
```

The public re-exports, copied from `lib.rs`:

```rust
pub use create::{
    AuthProvider, CreateSiteSpec, Database, Framework, JobId, JobState, JsRuntime, LaravelOptions,
    StarterKit, Testing,
};
pub use dump::{DumpCategory, DumpCounts, DumpEvent, DumpExtStatus};
pub use error::{FrameError, IpcError, IpcErrorKind};
pub use frame::{encode_frame, FrameDecoder, DEFAULT_MAX_FRAME};
pub use message::{decode_message, encode_message};
pub use request::Request;
pub use response::{ErrorCode, PhpUpdate, Response};
pub use status::{
    CaStatus, DatabaseSummary, Diagnosis, DiagnosisCode, FixReport, FixResult, MailDetail,
    MailHeader, MailStatus, MailSummary, PhpPoolStatus, PoolRunState, PortStatus,
    ServiceAvailability, ServiceRunState, ServiceStatus, Severity, SiteCounts, StatusReport,
    ToolStatus,
};

pub mod types {
    pub use yerd_core::{PhpVersion, Site, SiteKind};
}

#[cfg(feature = "transport")]
pub use transport::{read_frame, read_message, write_message};
```

The `types` module re-exports the [`yerd-core`](./yerd-core) types that travel on the wire so a client can depend on `yerd-ipc` alone (`use yerd_ipc::types::*;`) rather than pulling `yerd-core` directly.

## Purity and the `transport` feature

The headline design constraint is that **the default build is pure**: no sockets, no async runtime, no I/O. `Cargo.toml` makes `tokio` optional and gates it behind a feature:

```toml
[features]
default   = []
transport = ["dep:tokio"]
```

That split lets the codec be unit-tested without a runtime, lets the GUI link the message types without dragging `tokio` into its dependency graph if it doesn't need the helpers, and keeps the framing logic provably allocation- and side-effect-free. The daemon and CLI build with `--features transport` to get the async read/write helpers; everything else uses the pure surface.

Two further purity rules the crate enforces on itself: no `tracing` (the binaries own logging), and no `unwrap`/`expect`/`panic!`/indexing in non-test code (the workspace clippy gate denies them - even test modules carry an explicit `#[allow(...)]` block).

## The frame codec (`frame.rs`)

Every message is a length-prefixed frame: a 4-byte big-endian `u32` length followed by exactly that many payload bytes. The codec is **byte-agnostic** - it takes and returns `&[u8]` / `Vec<u8>` and never inspects payload contents. JSON encoding is an orthogonal layer (`message.rs`).

### `encode_frame`

```rust
pub fn encode_frame(payload: &[u8], max: usize) -> Result<Vec<u8>, FrameError>
```

Prepends the big-endian length and returns the framed bytes. `max` is **inclusive** (`payload.len() == max` is allowed). It fails with `FrameError::TooLarge` when the payload exceeds `max`, or `FrameError::PayloadOverflowsLengthPrefix` when the length does not fit in a `u32` (only reachable on 64-bit hosts). The sender capping with `max` lets it reject oversized payloads *before they hit the wire*; the receiver enforces its own cap independently. Both sides default to `DEFAULT_MAX_FRAME` (16 MiB) for symmetry.

### `FrameDecoder`

```rust
pub struct FrameDecoder { /* buf, max, poisoned */ }

impl FrameDecoder {
    pub fn new() -> Self;                                   // == with_max(DEFAULT_MAX_FRAME)
    pub fn with_max(max: usize) -> Self;
    pub fn with_max_and_capacity(max: usize, capacity: usize) -> Self;
    pub fn buffered(&self) -> usize;
    pub fn extend_from_slice(&mut self, chunk: &[u8]);
    pub fn next_frame(&mut self) -> Result<Option<Vec<u8>>, FrameError>;
}
```

The decoder is a small state machine. You feed it socket bytes with `extend_from_slice` and pull complete frames with `next_frame`:

- `Ok(Some(payload))` - one full frame is ready; any surplus bytes (from pipelined frames) stay buffered for the next call.
- `Ok(None)` - the header or body is still incomplete; feed more bytes.
- `Err(FrameError::TooLarge)` - the wire-declared length exceeds `max`.

It handles partial reads (header or body split across reads), multiple frames in a single buffer, and a hostile declared length.

::: warning Poisoning is permanent
When `next_frame` rejects an oversized declared length, the decoder is **poisoned**: it clears (and shrinks) its internal buffer to release memory, subsequent `next_frame` calls return the *same* error, and `extend_from_slice` becomes a no-op. Because `buffered()` then returns `0`, a transport helper reading a poisoned decoder at EOF may surface `IpcError::UnexpectedEof { bytes: 0 }`. There is no un-poison; a poisoned connection is dead.
:::

The length check itself is a tiny private helper, `check_payload_length(len, max) -> Result<u32, FrameError>`, shared by `encode_frame` and exercised directly by the unit tests (zero/zero, at-cap, one-over-cap, and the 64-bit `u32` overflow case).

## The message codec (`message.rs`)

```rust
pub fn encode_message<T: Serialize>(value: &T) -> Result<Vec<u8>, IpcError>;
pub fn decode_message<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, IpcError>;
```

These are thin `serde_json` wrappers that map `serde_json::Error` to `IpcError::Encode` / `IpcError::Decode`. Framing is entirely separate, so you can encode a message, inspect or log the JSON, and frame it later - or vice versa.

## Message envelopes

### `Request` (`request.rs`)

The client → daemon envelope is an internally tagged, `snake_case`, `#[non_exhaustive]` enum:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Request { /* ... */ }
```

The variants, grouped by area:

| Area | Variants |
| --- | --- |
| Liveness / info | `Ping`, `DaemonInfo`, `Status`, `Diagnose`, `DoctorFix`, `RestartDaemon` |
| Sites | `ListSites`, `Park`, `Link`, `Unlink`, `ListParked`, `Unpark`, `SetPhp`, `SetSecure`, `SetWebRoot` |
| PHP | `InstallPhp`, `InstallPhpStreamed`, `SetDefaultPhp`, `ListPhp`, `UpdatePhp`, `CheckPhpUpdates`, `AvailablePhp`, `SetPhpSettings`, `RestartPhp`, `RestartAllPhp`, `UninstallPhp` |
| Services | `ListServices`, `AvailableServices`, `InstallService`, `UninstallService`, `StartService`, `StopService`, `RestartService`, `SetServicePort`, `ServiceLogs`, `ChangeServiceVersion` |
| Databases | `CreateDatabase`, `ListDatabases`, `DropDatabase`, `BackupDatabase`, `RestoreDatabase` |
| Dumps (Laravel ▸ telemetry) | `ListDumps`, `ClearDumps`, `DeleteDump`, `SetDumpsEnabled`, `SetDumpsPort`, `SetDumpFeature`, `SetDumpsPersist`, `DumpsStatus` |
| Mail | `ListMails`, `GetMail`, `ClearMails`, `DeleteMails`, `SetMailPort`, `SetMailEnabled` |
| Tools | `ListTools`, `InstallTool`, `UninstallTool`, `InstallToolStreamed` |
| Site creation / jobs | `CreateSite`, `JobStatus`, `JobCancel` |

A few details that the source pins down and are worth knowing as a contributor:

- `Park`/`Link` carry a `PathBuf`; the path is opaque to `yerd-ipc` (the daemon canonicalises before storing) and Windows backslash paths are fine.
- `Unpark` carries a `String`, **not** a `PathBuf`, deliberately: the daemon stores parked roots as canonical `String`s in a `BTreeSet` and the client echoes a value straight back from `Response::Parked`, so an exact identity match avoids lossy `PathBuf` normalisation, and a folder deleted from disk stays removable.
- `UpdatePhp { version: Option<PhpVersion> }` - `Some` targets one minor, `None` means every installed version.
- `SetPhpSettings { settings: BTreeMap<String, String> }` - an empty-string value removes a key (resets it to PHP's built-in default).
- `SetWebRoot { name: String, path: Option<String> }` - sets a site's served web root (e.g. `"public"`); `None` resets it to auto-detection. The daemon validates the path resolves to a directory inside the site's document root.
- `RestartDaemon` is Unix-only; the daemon replies `Ok` *before* tearing down, then the connection closes as it re-execs.

### `Response` (`response.rs`)

Same shape - internally tagged, `snake_case`, `#[non_exhaustive]`:

| Variant | Replies to | Notable fields |
| --- | --- | --- |
| `Pong` | `Ping` | - |
| `Ok` | mutating requests (`Park`, `Link`, `Unlink`, `SetPhp`, `SetSecure`, `SetWebRoot`, …) | - |
| `Error` | any failure | `code: ErrorCode`, `message: String` |
| `Sites` | `ListSites` | `sites: Vec<Site>` (lexicographic) |
| `Parked` | `ListParked` | `paths: Vec<String>` |
| `Info` | `DaemonInfo` | `dns_addr`, `tld`, `ca_path`, `ca_fingerprint`, `http_port`, `https_port` |
| `PhpVersions` | `ListPhp` / `CheckPhpUpdates` / `UpdatePhp` | `installed`, `default`, `updates`, `settings` |
| `AvailablePhp` | `AvailablePhp` | `available`, `installed` |
| `Status` | `Status` | `report: Box<StatusReport>` |
| `Diagnoses` | `Diagnose` | `items: Vec<Diagnosis>` |
| `DoctorFix` | `DoctorFix` | `report: FixReport` |
| `Services` | `ListServices` | `services: Vec<ServiceStatus>` |
| `AvailableServices` | `AvailableServices` | `services: Vec<ServiceAvailability>` |
| `ServiceLogs` | `ServiceLogs` | `lines: Vec<String>` |
| `Databases` | `ListDatabases` | `databases: Vec<DatabaseSummary>` |
| `Dumps` | `ListDumps` | `events: Vec<DumpEvent>` + deleted ids (cursor paging) |
| `DumpsStatus` | `DumpsStatus` / dump toggles | `enabled`, `port`, persist/feature flags |
| `Mails` | `ListMails` | `mails: Vec<MailSummary>` |
| `Mail` | `GetMail` | `Box<MailDetail>` |
| `Tools` | `ListTools` | `tools: Vec<ToolStatus>` |
| `JobStarted` | `CreateSite` / `InstallToolStreamed` / `InstallPhpStreamed` | `job_id: JobId` (poll with `JobStatus`) |
| `JobProgress` | streamed job updates | `state: JobState`, phase label, … |

Notable behaviours:

- `Status` boxes its report (`Box<StatusReport>`) so the large payload doesn't bloat every `Response` value. `Box<T>` serializes transparently, so the wire bytes are unchanged.
- `Info.http_port` / `Info.https_port` are `#[serde(default)]` (defaulting to `0`) so an older daemon that omits them stays decodable by a newer client.
- `PhpVersions.updates` and `PhpVersions.settings` are `#[serde(default, skip_serializing_if = ...)]` - empty collections vanish from the wire, keeping the bytes identical to the pre-field shape.

### `ErrorCode`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ErrorCode { NotFound, AlreadyExists, InvalidPath, PortInUse, Internal }
```

`code` is machine-readable; `message` is for human display. There is **no** `#[serde(other)]` catch-all: an unknown code from a newer daemon fails closed as `IpcError::Decode`, which is the broad "version mismatch" signal until a handshake exists. `Internal` is the catch-all for daemon-side failures that don't fit a typed code - the guidance in the source is to *expand* the enum rather than overload `Internal`.

### `PhpUpdate`

```rust
pub struct PhpUpdate { pub version: PhpVersion, pub installed: String, pub latest: String }
```

One entry per installed minor that has a newer published patch (e.g. version `8.5`, installed `8.5.6`, latest `8.5.7`).

## Dump telemetry (`dump.rs`)

The Laravel ▸ Dumps feature ships per-request telemetry from the `yerd-php-ext` extension to the daemon's loopback dump server; the daemon buffers it and serves it to the GUI over IPC. `dump.rs` holds the shared data model. The daemon treats each event's payload as **opaque** JSON, so the extension's payload schema can evolve without daemon changes; the GUI renders it per category. Wire shapes are pinned in `tests/wire_stability.rs`.

| Type | Shape | Role |
| --- | --- | --- |
| `DumpCategory` | `snake_case`, `#[non_exhaustive]` enum: `Dump`, `Query`, `Job`, `View`, `Request`, `Log`, `Cache`, `Http` | One per GUI tab; the category of a captured frame. `Copy + Ord`. |
| `DumpEvent` | `{ id: u64, category: DumpCategory, ts_ms: u64, site: String, request_id: String, payload: serde_json::Value }` | One buffered event. `id` is assigned by the daemon (clients page with `since_id`); `payload` is the opaque category-specific JSON. |
| `DumpCounts` | `{ dumps, queries, jobs, views, requests, logs, cache, http: u32 }` | Per-category counts of events currently in the daemon's ring (capacity ~2000, so `u32` not `u64`). `increment(category)` bumps the matching field. `Copy + Eq`. |
| `DumpExtStatus` | `{ version: PhpVersion, present: bool }` | Whether a matching extension `.so` is present for an installed PHP version (a yerd-side "artifact present and wired" fact, not proof FPM `dlopen`'d it). `Eq`. |

::: warning `DumpEvent` is not `Eq` - and that ripples up to `Response`
`DumpEvent::payload` is a `serde_json::Value`, which can hold floats, so `DumpEvent` derives `PartialEq` but **not** `Eq`. Because a `DumpEvent` is reachable from `Response`, `Response` itself **no longer derives `Eq`** (only `PartialEq`). `Request`, which reaches no float-bearing type, still derives `PartialEq + Eq`. See the [No `f64` on the wire](#no-f64-on-the-wire-note) note below for what this changes.
:::

## Status & doctor payloads (`status.rs`)

These types ride inside `Response::Status`, `Response::Diagnoses`, `Response::DoctorFix`, and the service/database responses. The headline list:

`StatusReport`, `PortStatus`, `CaStatus`, `SiteCounts`, `PhpPoolStatus`, `PoolRunState`, `ServiceStatus`, `ServiceRunState`, `ServiceAvailability`, `DatabaseSummary`, `Diagnosis`, `Severity`, `DiagnosisCode`, `FixReport`, `FixResult`.

::: info No `f64` on the status payload {#no-f64-on-the-wire-note}
`StatusReport` and everything it reaches stays float-free even though `Response` no longer derives `Eq` (the `DumpEvent::payload` `serde_json::Value` forced that to `PartialEq`-only - see [Dump telemetry](#dump-telemetry-dump-rs)). The system load average still crosses as integer hundredths - `StatusReport.load_avg` is `Option<[u32; 3]>` where each value is `load × 100`. The CLI renders it back to `x.xx`. The daemon does the conversion from the platform layer's `f64` reading at assembly time. Keeping the *status* payload integer-only preserves its `Eq`-based golden assertions; the only intentional float on `Response` is the opaque, daemon-uninterpreted dump payload.
:::

`StatusReport` follows the same additive-and-back-compatible discipline as the envelopes: optional probe fields (`port_redirect` and the cross-platform `foreign_web_listener`, plus macOS-only `resolver_backup`) are `#[serde(default, skip_serializing_if = "Option::is_none")]` so the wire stays additive and older daemons stay decodable; `daemon_version` is `#[serde(default)]` so a newer client decoding an older daemon's status gets `""` (rendered "unknown") instead of failing the whole decode.

`PoolRunState` is `Running` / `Stopped` / `Failed`; `ServiceRunState` is `Running` / `Stopped` / `Failed`; `Severity` is `Ok` / `Warn` / `Fail`; `DiagnosisCode` enumerates the doctor checks (`DaemonDown`, `PortFallback`, `ForeignWebListener`, `CaNotTrusted`, `ResolverNotInstalled`, `NoPhpInstalled`, `DefaultPhpNotInstalled`, `FpmPoolFailed`, `ServiceFailed`, `PhpUpdateAvailable`, `NoSites`, `ResolverBackupSaved`, `AllGood`). See [Diagnostics](../../guide/diagnostics) for what these mean to a user.

## Errors (`error.rs`)

Two layers, split on cloneability:

```rust
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum FrameError {
    TooLarge { size: u64, max: u64 },
    PayloadOverflowsLengthPrefix { size: u64 },
}
```

`FrameError` is the pure framing error and is `Clone + Eq`. `IpcError` is the top-level error:

```rust
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum IpcError {
    Encode(#[source] serde_json::Error),
    Decode(#[source] serde_json::Error),
    Frame(#[from] FrameError),
    UnexpectedEof { bytes: usize },
    Io { kind: std::io::ErrorKind },
}
```

`IpcError` is **not** `Clone`/`Eq` because `serde_json::Error` isn't. `UnexpectedEof` and `Io` are produced *only* by the `transport` helpers (the pure codec cannot synthesise them); `Io` carries `std::io::ErrorKind` (which is `Copy + Eq`) rather than the non-cloneable `std::io::Error`.

For callers that need a `Clone + Eq` value - notably Tauri commands, which serialize their `Result` error into the GUI - `IpcError::kind()` returns a pattern-matchable shadow:

```rust
pub fn kind(&self) -> IpcErrorKind;   // Clone + Eq mirror
pub fn message(&self) -> String;      // == Display, allocates once
```

`IpcErrorKind` mirrors every `IpcError` variant and additionally has a `FrameOther { description }` catch-all that carries the `Display` rendering of any *future* `FrameError` variant added before its paired `IpcErrorKind` variant lands. A unit test, `frame_error_to_kind_is_exhaustive`, matches exhaustively on the in-crate `FrameError` and asserts each maps to a concrete (non-`FrameOther`) kind - so the catch-all is unreachable today and the pairing can't silently drift.

## The transport layer (`transport.rs`, feature-gated)

Behind `--features transport`, three helpers are generic over `tokio::io::AsyncRead` / `AsyncWrite` so the daemon and CLI share one read/write path while socket and named-pipe binding stays in the binaries:

```rust
pub async fn write_message<W, T>(writer: &mut W, value: &T, max: usize) -> Result<(), IpcError>
where W: AsyncWrite + Unpin, T: Serialize;

pub async fn read_frame<R>(reader: &mut R, decoder: &mut FrameDecoder)
    -> Result<Option<Vec<u8>>, IpcError>
where R: AsyncRead + Unpin;

pub async fn read_message<R, T>(reader: &mut R, decoder: &mut FrameDecoder)
    -> Result<Option<T>, IpcError>
where R: AsyncRead + Unpin, T: DeserializeOwned;
```

`write_message` encodes → frames → `write_all`. `read_frame` loops: drain a ready frame from the decoder, else `read` a 4 KiB chunk and feed it in. Its three EOF-adjacent outcomes are precise:

- `Ok(Some(payload))` - a full frame.
- `Ok(None)` - clean EOF with an empty decoder buffer.
- `Err(IpcError::UnexpectedEof { bytes })` - EOF arrived mid-frame (`bytes` were buffered).
- `Err(IpcError::Frame(_))` - declared length exceeded the cap; the decoder is now poisoned.

`read_message` is `read_frame` followed by `decode_message`. The caller can therefore inspect the raw `type` tag from `read_frame` before committing to a full decode. The private `io_to_ipc` helper preserves only the OS error *category* (`ErrorKind`), keeping `IpcErrorKind` cloneable; clean EOF (a zero-byte read) is handled separately so it never masquerades as an `Io` error.

## Wire-stability policy

The `Request` / `Response` / `ErrorCode` JSON shapes are a **published contract**. Three mechanisms enforce it, and breaking any of them fails CI before a divergent format reaches a client:

1. **Add additively, never rename.** `#[non_exhaustive]` allows new variants without a major bump. Renaming a variant, field, or error code is forbidden.
2. **No per-field `#[serde(rename = "...")]`.** Casing is `#[serde(rename_all = "snake_case")]` only. A grep gate fails on any per-field rename in `crates/yerd-ipc/src/`. The reason is subtle: pairing the no-rename rule with the byte-pin tests means a Rust variant rename trips *both* the wire pin (changed JSON) and the in-crate exhaustive match (compile error) - a stray `#[serde(rename = ...)]` could otherwise mask a Rust rename while silently breaking clients.
3. **Byte-exact pinning.** `tests/wire_stability.rs` asserts the literal JSON for every variant. Inline `variant_name_pinning` modules in `request.rs` / `response.rs` carry an exhaustive `match` over every variant - these live *inside* the crate because `#[non_exhaustive]` blocks exhaustive matching from an integration test across the crate boundary.

`PROTOCOL_VERSION` is `1` and is reserved: there is no `Hello`/`Welcome` handshake yet, so a newer client against an older daemon surfaces an unknown `type` tag as `IpcError::Decode`. Don't bump the version without landing the paired handshake variants first.

### Envelope-permissive, payload-strict

A deliberate asymmetry, asserted in `tests/roundtrip.rs`:

- The **outer envelope** (`Request` / `Response`) **accepts** unknown JSON fields, so additive field changes stay backward-compatible. `{"type":"ping","__extra":42}` decodes as `Request::Ping`.
- The **inner `Site`** payload is **strict** (`yerd_core::Site` uses `deny_unknown_fields`). An unknown field on a `Site` inside `Response::Sites` is rejected.

Unknown `type` tags and unknown `ErrorCode` values both fail closed as `IpcError::Decode` - there is no silent-downgrade catch-all.

## Tests and invariants

| File | Covers |
| --- | --- |
| `tests/frame_codec.rs` | partial header/body reads, pipelined frames, oversized rejection, decoder poisoning, exact-max boundary, empty payload (`[0,0,0,0]`), slow-loris byte-at-a-time |
| `tests/wire_stability.rs` | byte-exact JSON for every `Request`/`Response`/`ErrorCode`/`DiagnosisCode`/`Severity`/`PoolRunState` variant, plus back-compat (legacy `Info` without ports, `skip_serializing_if` field omission) |
| `tests/roundtrip.rs` | `encode_message` ∘ `decode_message` identity; negative tests for unknown tag, missing required field, unknown envelope field (accepted), unknown `Site` field (rejected), unknown `ErrorCode` (rejected) |
| inline `src/*.rs` | `check_payload_length` edge cases, `IpcError::kind` exhaustiveness, `Display` parity, `PROTOCOL_VERSION` pin, variant-name pinning, async transport over an in-memory `duplex` |

The async transport tests (`transport.rs`) run on an in-memory `tokio::io::duplex`, covering a full round-trip, a frame split across two writes with a yield between, clean EOF, mid-frame EOF, a write to a closed reader surfacing as `IpcError::Io`, and partial writes over a tiny buffer.

::: details Local verification gate
```sh
cargo build -p yerd-ipc
cargo build -p yerd-ipc --features transport
cargo test  -p yerd-ipc
cargo test  -p yerd-ipc --features transport
cargo fmt   --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
# fail on any per-field serde rename in this crate's src/
! grep -REn '#\[serde\([^)]*[^_[:alnum:]]?rename[[:space:]]*=' crates/yerd-ipc/src/
```
:::

## Related

- [IPC Protocol](../ipc-protocol) - the full protocol deep-dive (connection lifecycle, choreography, version skew).
- [yerd-core](./yerd-core) - the `Site`, `PhpVersion`, and `SiteKind` types re-exported through `yerd_ipc::types`.
- [yerdd (daemon)](../binaries/yerdd) and [yerd (CLI)](../binaries/yerd) - the `transport`-feature consumers that bind the actual sockets.
- [Crates Overview](../crates) - where this crate sits in the workspace.
