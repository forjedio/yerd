# IPC Protocol

This page is the contributor-facing reference for the protocol spoken between the daemon (`yerdd`) and its clients - the `yerd` CLI and the Tauri desktop app. Everything here lives in the [`yerd-ipc`](./crates/yerd-ipc) crate (`crates/yerd-ipc/`), with socket/pipe binding deferred to the binaries.

The crate has three jobs, layered so they can be tested in isolation:

```
  message model      Request / Response / ErrorCode + status payloads   (always built)
  JSON codec         encode_message / decode_message (serde_json)        (always built)
  frame codec        encode_frame / FrameDecoder (4-byte length prefix)  (always built)
  ----------------------------------------------------------------------------------
  transport          read_frame / read_message / write_message (tokio)   (feature = "transport")
  socket / pipe      interprocess local-socket bind + connect            (lives in the binaries)
```

The top three layers are **pure**: no sockets, no async, no I/O. The `transport` feature pulls in `tokio` async helpers; the actual socket/named-pipe binding never enters the crate at all - it lives in `yerdd`, `yerd`, and the Tauri sidecar.

::: info Source map
`crates/yerd-ipc/src/`: `lib.rs` (re-exports + `PROTOCOL_VERSION`), `frame.rs` (framing), `message.rs` (JSON codec), `request.rs` / `response.rs` / `status.rs` (the wire types), `dump.rs` (the Laravel ▸ Dumps data model: `DumpCategory`, `DumpEvent`, `DumpCounts`, `DumpExtStatus`), `error.rs` (`FrameError` / `IpcError`), `transport.rs` (feature-gated async helpers). Tests: `tests/frame_codec.rs`, `tests/wire_stability.rs`, `tests/roundtrip.rs`. Browse them on [GitHub](https://github.com/forjedio/yerd).
:::

## Transport

The default build of `yerd-ipc` has no transport at all. Enable it explicitly:

```toml
# yerdd / yerd / the Tauri backend depend on it like this
yerd-ipc = { path = "../../crates/yerd-ipc", features = ["transport"] }
```

In the crate's own `Cargo.toml`, `tokio` is `optional = true` and the feature simply turns it on:

```toml
[features]
default   = []
transport = ["dep:tokio"]
```

Keeping the codec runtime-free means tests of framing and wire shapes compile and run with no async runtime, and any consumer that only needs the *types* (for example, to share `Request`/`Response` definitions) pays nothing for `tokio`.

The transport helpers in `transport.rs` are generic over `tokio::io::AsyncRead` / `AsyncWrite` - they never name a concrete socket type. The concrete binding is done in the binaries with the [`interprocess`](https://crates.io/crates/interprocess) crate's `local_socket`, which maps to:

- **Unix domain socket** on macOS and Linux - `<runtime>/yerd.sock`, where `<runtime>` is resolved by `yerd-platform` (with the `/tmp/yerd-$UID` fallback when `XDG_RUNTIME_DIR` is unset). The daemon removes any stale socket file, then `restrict_to_owner`s it (`0o700` runtime dir plus an owner-only socket) because the IPC server performs no peer-credential check - file permissions are the access boundary.
- **Named pipe** on Windows - currently `yerd-<pid>` via `GenericNamespaced`.

The daemon side (`bin/yerdd/src/startup.rs`) selects the name per OS:

```rust
#[cfg(unix)]
let socket_path = dirs.runtime.join("yerd.sock");
// ... to_fs_name::<GenericFilePath>() ...
#[cfg(windows)]
let name = {
    let pipe = format!("yerd-{}", std::process::id());
    pipe.clone().to_ns_name::<GenericNamespaced>()? // namespaced pipe
};
let listener = ListenerOptions::new().name(name).create_tokio()?;
```

::: warning Windows client is not wired up yet
The `yerd` CLI derives the Unix socket path *identically* to the daemon, so they always agree. The Windows pipe name is PID-based and therefore not derivable by a client; the CLI returns `ClientError::DaemonUnreachable` on non-Unix targets today. This is tracked as a follow-up - treat full Windows client support as roadmap. See [Cross-Platform Model](./cross-platform).
:::

The accept loop is one `tokio::spawn` per connection, and a connection is a long-lived request/response stream - the daemon reads a `Request`, dispatches it, writes a `Response`, and loops until EOF (`bin/yerdd/src/ipc_server.rs`). The CLI typically does a single exchange and drops the connection.

## Framing

Every message is one **length-prefixed frame**: a 4-byte big-endian `u32` length followed by exactly that many payload bytes. The codec is byte-agnostic - it takes and returns `&[u8]` / `Vec<u8>` and never inspects the payload - so framing and the JSON codec are fully orthogonal.

```
+----------------+--------------------------------+
| len: u32 (BE)  | payload: `len` bytes           |
+----------------+--------------------------------+
   4 bytes          0 .. DEFAULT_MAX_FRAME
```

The maximum frame size is **16 MiB**:

```rust
/// 16 MiB - the default maximum frame size on both sides.
pub const DEFAULT_MAX_FRAME: usize = 16 * 1024 * 1024;
```

### Encoding

`encode_frame` validates the payload length against a caller-supplied `max` (inclusive - `len == max` is allowed) and prepends the big-endian length:

```rust
pub fn encode_frame(payload: &[u8], max: usize) -> Result<Vec<u8>, FrameError>;
```

It fails with `FrameError::TooLarge { size, max }` if the payload exceeds `max`, or `FrameError::PayloadOverflowsLengthPrefix { size }` if the length does not fit in the 4-byte prefix (only reachable on 64-bit hosts). Both sides default to `DEFAULT_MAX_FRAME`; the sender's cap lets it reject an oversized payload before it ever hits the wire, while the receiver enforces its own cap independently.

### Decoding - `FrameDecoder`

`FrameDecoder` is the stateful read side. You feed it socket bytes and pull complete frames:

```rust
let mut dec = FrameDecoder::new();          // DEFAULT_MAX_FRAME cap
dec.extend_from_slice(&chunk);              // append raw socket bytes
match dec.next_frame()? {
    Some(payload) => { /* one full frame; surplus stays buffered */ }
    None          => { /* header or body incomplete - read more */ }
}
```

It is built to survive the realities of stream sockets, all pinned in `tests/frame_codec.rs`:

| Situation | Behaviour |
| --- | --- |
| **Partial header** (< 4 bytes) | `next_frame()` returns `Ok(None)` |
| **Partial body** (header read, body short) | `Ok(None)` |
| **Multiple frames in one buffer** (pipelined) | successive `next_frame()` calls drain them in order |
| **Trailing surplus bytes** | kept buffered for the next frame; `buffered()` reflects the count |
| **Slow-loris, one byte at a time** | reassembles correctly over many `extend_from_slice` calls |
| **Declared length > `max`** | `Err(FrameError::TooLarge { size, max })`, decoder is **poisoned** |

::: details Decoder poisoning
When `next_frame` rejects an oversized declared length, the decoder is *poisoned*: it clears (and shrinks) its internal buffer to release memory, every later `next_frame()` returns the same `TooLarge` error, and `extend_from_slice` becomes a no-op. Because `buffered()` then returns 0, a subsequent EOF on a poisoned decoder surfaces through the transport layer as `IpcError::UnexpectedEof { bytes: 0 }`. Tested by `decoder_stays_poisoned_after_oversized`.
:::

An empty payload is a valid frame - `encode_frame(b"", _)` yields exactly `[0, 0, 0, 0]`.

## JSON codec

`message.rs` is a thin pair of `serde_json` wrappers that map errors into the IPC error space:

```rust
pub fn encode_message<T: Serialize>(value: &T) -> Result<Vec<u8>, IpcError>;     // -> IpcError::Encode
pub fn decode_message<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, IpcError>; // -> IpcError::Decode
```

The framing layer carries this UTF-8 JSON; the two never depend on each other.

## Message model

Both envelopes are `enum`s, **internally tagged on `type`**, `snake_case`, and `#[non_exhaustive]`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Request { /* ... */ }
```

`#[non_exhaustive]` means consumers in other crates cannot exhaustively match, so new variants are additive without a breaking change.

The two derives differ in one respect: `Request` derives `Eq`, but **`Response` does not** - it is `PartialEq` only. `Response::Dumps` carries `DumpEvent`s whose `payload` is an opaque `serde_json::Value` (which can hold floats, so it is `PartialEq` but not `Eq`). `PartialEq` is all the wire-stability round-trips need on the response side.

### Request (client → daemon)

The variant set is the daemon's whole RPC surface - liveness, site management, PHP version management, **database/cache service management**, **SQL database administration**, **mail capture**, **dump telemetry**, **Cloudflare Tunnel sharing**, status/doctor, and daemon lifecycle. A representative sample with its exact wire shape (the full list of tags lives in `request.rs`):

| Variant | Wire JSON |
| --- | --- |
| `Ping` | `{"type":"ping"}` |
| `ListSites` | `{"type":"list_sites"}` |
| `Park { path }` | `{"type":"park","path":"/srv/foo"}` |
| `Link { name, path }` | `{"type":"link","name":"foo","path":"/srv/foo"}` |
| `SetPhp { name, version }` | `{"type":"set_php","name":"foo","version":"8.3"}` |
| `SetSecure { name, secure }` | `{"type":"set_secure","name":"foo","secure":true}` |
| `SetWebRoot { name, path: Option }` | `{"type":"set_web_root","name":"foo","path":"public"}` or `…,"path":null` (reset to auto-detect) |
| `AddDomain { name, domain }` | `{"type":"add_domain","name":"foo","domain":"api.foo.test"}` (exact host or single-label wildcard `*.foo.test`) |
| `RemoveDomain { name, domain }` | `{"type":"remove_domain","name":"foo","domain":"api.foo.test"}` (refused for a site's last exact domain) |
| `SetPrimaryDomain { name, domain }` | `{"type":"set_primary_domain","name":"foo","domain":"corp.foo.test"}` (exact host only, never a wildcard; auto-added if absent) |
| `ResetDomains { name }` | `{"type":"reset_domains","name":"foo"}` (back to apex only) |
| `InstallPhp { version }` | `{"type":"install_php","version":"8.5"}` |
| `InstallPhpStreamed { version }` | `{"type":"install_php_streamed","version":"8.5"}` (replies `JobStarted`; poll `JobStatus`) |
| `UpdatePhp { version: Option }` | `{"type":"update_php","version":"8.5"}` or `…,"version":null` |
| `SetPhpSettings { settings }` | `{"type":"set_php_settings","settings":{…}}` |
| `AddPhpExtension { version, path, name: Option, zend }` | `{"type":"add_php_extension","version":"8.5","path":"/a/scrypt.so","name":null,"zend":false}` |
| `RemovePhpExtension { version, name }` | `{"type":"remove_php_extension","version":"8.5","name":"scrypt"}` |
| `ListPhpExtensions` | `{"type":"list_php_extensions"}` (replies `PhpExtensions { by_version }`) |
| `ListServices` / `AvailableServices` | `{"type":"list_services"}` / `{"type":"available_services"}` |
| `InstallService { service, version }` | `{"type":"install_service","service":"redis","version":"8"}` |
| `ChangeServiceVersion { service, version }` | `{"type":"change_service_version","service":"redis","version":"8.1"}` |
| `UninstallService { service, version, purge }` | `{"type":"uninstall_service","service":"redis","version":"8","purge":false}` |
| `StartService` / `StopService` / `RestartService` | `{"type":"start_service","service":"redis"}` (and `stop_`/`restart_`) |
| `SetServicePort { service, port }` | `{"type":"set_service_port","service":"redis","port":6380}` |
| `ServiceLogs { service, lines }` | `{"type":"service_logs","service":"redis","lines":100}` |
| `ListDatabases { service }` | `{"type":"list_databases","service":"mysql"}` |
| `CreateDatabase` / `DropDatabase` | `{"type":"create_database","service":"mysql","name":"app"}` (and `drop_database`) |
| `BackupDatabase { service, name, path }` | `{"type":"backup_database","service":"mysql","name":"app","path":"/tmp/app.sql"}` |
| `RestoreDatabase { service, name, path }` | `{"type":"restore_database","service":"mysql","name":"app","path":"/tmp/app.sql"}` |
| `Status` | `{"type":"status"}` |
| `Diagnose` / `DoctorFix` | `{"type":"diagnose"}` / `{"type":"doctor_fix"}` |
| `RestartDaemon` | `{"type":"restart_daemon"}` (Unix-only re-exec) |
| `ListMails` | `{"type":"list_mails"}` |
| `GetMail { id }` | `{"type":"get_mail","id":"000001"}` |
| `ClearMails` | `{"type":"clear_mails"}` |
| `DeleteMails { ids }` | `{"type":"delete_mails","ids":["000001"]}` |
| `MarkMailsRead { ids }` | `{"type":"mark_mails_read","ids":["000001"]}` |
| `SetMailPort { port }` | `{"type":"set_mail_port","port":2525}` |
| `SetMailEnabled { enabled }` | `{"type":"set_mail_enabled","enabled":true}` |
| `ListDumps { since_id }` | `{"type":"list_dumps","since_id":0}` |
| `ClearDumps` | `{"type":"clear_dumps"}` |
| `DeleteDump { id }` | `{"type":"delete_dump","id":1}` |
| `SetDumpsEnabled { enabled }` | `{"type":"set_dumps_enabled","enabled":true}` |
| `SetDumpsPort { port }` | `{"type":"set_dumps_port","port":2304}` |
| `SetDumpFeature { feature, enabled }` | `{"type":"set_dump_feature","feature":"queries","enabled":true}` |
| `SetDumpsPersist { persist }` | `{"type":"set_dumps_persist","persist":true}` |
| `DumpsStatus` | `{"type":"dumps_status"}` |
| `ListTools` | `{"type":"list_tools"}` |
| `InstallTool { tool }` | `{"type":"install_tool","tool":"node"}` |
| `UninstallTool { tool }` | `{"type":"uninstall_tool","tool":"bun"}` |
| `CreateSite { spec }` | `{"type":"create_site","spec":{"name":"blog","parent_dir":"/srv","php":"8.3","secure":false,"framework":{"framework":"wordpress",…}}}` (replies `JobStarted`; poll `JobStatus`) |
| `AvailableWordpressVersions` | `{"type":"available_wordpress_versions"}` |
| `MintWordpressLoginToken { site }` | `{"type":"mint_wordpress_login_token","site":"blog"}` |
| `SetWordpressAutoLogin { name, enabled, user: Option }` | `{"type":"set_wordpress_auto_login","name":"blog","enabled":true,"user":"admin"}` |
| `WordpressAdminUsers { site }` | `{"type":"wordpress_admin_users","site":"blog"}` |

Note `Unpark { path: String }` deliberately uses a `String`, not `PathBuf`: clients echo a value straight back from `Response::Parked`, and an exact-identity match avoids lossy path normalisation (the daemon does not canonicalise it).

`CreateSite { spec: CreateSiteSpec }` is the wizard's entry point (both Laravel and WordPress): `CreateSiteSpec` carries the shared fields (`name`, `parent_dir`, `php`, `secure`) plus a `framework: Framework` enum internally tagged on `"framework"` (`"laravel"` with `LaravelOptions`, `"wordpress"` with `WordPressOptions` - core version, locale, admin credentials, database engine/name/table-prefix). It replies `JobStarted { job_id }` immediately; the caller polls `Request::JobStatus { job_id }` (streamed via `Response::JobProgress`) the same way `InstallPhpStreamed`/`InstallToolStreamed` do. See [`yerdd`'s WordPress support section](./binaries/yerdd#wordpress-support) for what runs behind the job.

### Response (daemon → client)

```rust
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Response {
    Pong,
    Ok,                                           // generic mutation success
    Sites { sites: Vec<Site> },
    Error { code: ErrorCode, message: String },
    Parked { paths: Vec<String> },
    Info { dns_addr, tld, ca_path, ca_fingerprint, http_port, https_port },
    PhpVersions { installed, default, updates, settings },
    AvailablePhp { available, installed },
    PhpExtensions { by_version },                  // version → [PhpExtInfo{name,path,zend,present}]
    Status { report: Box<StatusReport> },         // boxed: large payload
    Diagnoses { items: Vec<Diagnosis> },
    DoctorFix { report: FixReport },
    Services { services: Vec<ServiceStatus> },
    AvailableServices { services: Vec<ServiceAvailability> },
    ServiceLogs { lines: Vec<String> },
    Databases { databases: Vec<DatabaseSummary> },
    Mails { mails: Vec<MailSummary> },
    Mail { mail: Box<MailDetail> },               // boxed: large payload
    Tools { tools: Vec<ToolStatus> },             // installable dev tools
    WordpressVersions { versions: Vec<WordPressVersionInfo> },
    WordpressLoginToken { token: String },
    WordpressAdminUsers { users: Vec<WordPressAdminUser> },
    Dumps {
        events: Vec<DumpEvent>,
        removed_ids: Vec<u64>,
        counts: DumpCounts,
        latest_id: u64,
        min_live_id: u64,
    },
    DumpsStatus {
        enabled: bool,
        port: u16,
        running: bool,
        persist: bool,
        extensions: Vec<DumpExtStatus>,
        counts: DumpCounts,
        features: BTreeMap<String, bool>,
    },
    JobStarted { job_id: JobId },                 // CreateSite / InstallToolStreamed / InstallPhpStreamed
    JobProgress { state: JobState, /* phase label, … */ },  // streamed updates
}
```

`Response::Ok` is the generic success for mutating requests (`Park`, `Link`, `Unlink`, `SetPhp`, `SetSecure`, `SetWebRoot`, …). The large `StatusReport` is boxed so it does not bloat every `Response` value; `Box<T>` serializes transparently, so the wire bytes are unchanged.

::: info The `Site` payload gained a field additively
`Site` (inside `Response::Sites`) gained an optional `web_subpath` after `document_root`. It is **skipped when empty**, so a root-served site's JSON is byte-identical to before the field existed - the wire-stability goldens for the empty case are unchanged, and only the non-empty case (`"web_subpath":"public"`) added a new pin. Old clients ignore the field; no `PROTOCOL_VERSION` bump was needed.
:::

::: info WordPress fields on `Site` and `SiteEntry`
`Site` gained `wp_auto_login: bool` and `wp_auto_login_user: Option<String>`, both skipped on the wire when absent/`false` - same additive, no-bump pattern as `web_subpath`. `Response::Sites`'s per-entry payload (`SiteEntry`, `#[serde(flatten)]` over `Site`) separately gained `is_wordpress: bool`, also skipped when `false` - it's a runtime detection fact (see `wordpress_detect`, [`yerdd`'s WordPress support](./binaries/yerdd#wordpress-support)), not a persisted config field, so it lives on the response wrapper rather than the config-backed `Site` itself.
:::

::: info Domain fields on `SiteEntry`, and `StatusReport.shadows`
The multi-domain feature adds three more `SiteEntry` fields alongside `is_wordpress`: `primary_domain: Option<String>` (the canonical FQDN, populated only when it differs from the default `{name}.{tld}` apex), `domains: Vec<String>` (the full routable set, populated only for a customized site), and `apex_shadowed_by: Option<String>` (the other site claiming this apex, if any). All three are skipped when default/empty, so a default site's `Response::Sites` bytes are byte-identical to before - same additive, no-bump pattern as `web_subpath`. Separately, `StatusReport` gained `shadows: Vec<DomainShadow>` (skipped when empty): one `DomainShadow { site, shadowed_by }` per site that lost a domain to another when the router was built, which `yerd doctor` surfaces as a `DomainShadowed` warning. The four domain mutators (`AddDomain` / `RemoveDomain` / `SetPrimaryDomain` / `ResetDomains`) reply with the generic `Ok`.
:::

::: info The mail payloads gained read/unread fields additively
For read/unread tracking, `MailSummary` gained a `read: bool` (last field) and `MailStatus` gained an `unread: u32` (last field), both `#[serde(default)]`. A missing key decodes to `false`/`0`, so old daemons and old clients interoperate without a `PROTOCOL_VERSION` bump; the goldens grew a `,"read":false` / `,"unread":0` suffix. The matching mutator is the additive `Request::MarkMailsRead { ids }`.
:::

::: info Reverse proxies are additive requests plus their own response variant
The proxy feature adds four mutators - `AddProxy { name, url }`, `RemoveProxy { name }`, `AddProxyRule { site, prefix, url }`, `RemoveProxyRule { site, prefix }` (all reply with the generic `Ok`) - and one query, `ListProxies`. Because a whole-host proxy is **not** a `Site`, it can't ride `Response::Sites`/`SiteEntry`; `ListProxies` gets a dedicated `Response::Proxies { proxies: Vec<ProxyEntry>, rules: Vec<ProxyRuleEntry> }` instead, where `ProxyEntry { name, target, secure }` and `ProxyRuleEntry { site, prefix, target }` are all `String`/`bool` so the `Response` `Eq` derive holds. New variants are additive by serde tag, so existing pins are byte-identical and no `PROTOCOL_VERSION` bump is needed. `url` stays a `String` on the wire; the daemon parses and validates it (returning a typed error) rather than the client.
:::

### ErrorCode

Failures are a `Response::Error { code, message }` where `code` is machine-readable and `message` is for human display:

```rust
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ErrorCode {
    NotFound,             // "not_found"
    AlreadyExists,        // "already_exists"
    InvalidPath,          // "invalid_path"
    PortInUse,            // "port_in_use"
    ExtensionLoadFailed,  // "extension_load_failed" - a valid path whose .so failed its load-probe
    Internal,             // "internal" - catch-all; expand the enum, don't overload this
}
```

There is deliberately **no `#[serde(other)]` catch-all**. An unknown code from a newer daemon fails closed as `IpcError::Decode` rather than silently downgrading - the same signal as an unknown `type` tag, and the placeholder for a real version mismatch until a handshake lands.

### Status & doctor payloads

`status.rs` holds the nested payloads carried inside the status/doctor, service/database, and mail responses: `StatusReport`, `PortStatus`, `CaStatus`, `SiteCounts`, `PhpPoolStatus`, `PoolRunState`, `ServiceStatus`, `ServiceRunState`, `ServiceAvailability`, `DatabaseSummary`, `Diagnosis`, `Severity`, `DiagnosisCode`, `FixReport`, `FixResult`, and the mail-capture types `MailStatus`, `MailSummary`, `MailHeader`, and `MailDetail`. Same contract rules apply. `StatusReport` also carries an additive `services: Vec<ServiceStatus>` field alongside the PHP pools, plus an additive `mail: Option<MailStatus>`.

`dump.rs` holds the dump-telemetry payloads carried inside the `Dumps` / `DumpsStatus` responses: `DumpCategory` (the per-tab category enum), `DumpEvent` (one buffered event; its `payload` is an opaque `serde_json::Value`), `DumpCounts` (per-category buffered counts), and `DumpExtStatus` (per-PHP-version extension presence). Same contract rules apply.

::: tip Integer-encoded scalars (the load average)
`Request` derives `Eq`, but `Response` does **not** - the `Dumps` response carries arbitrary JSON dump payloads (a `serde_json::Value`, which may hold floats), so an `Eq` derive is impossible there. That float escape hatch is confined to opaque dump payloads, though: the daemon never interprets them. The daemon's *own* status scalars are still integer-encoded so they stay exact and comparable. The system load average therefore crosses as integer hundredths - `StatusReport::load_avg` is `Option<[u32; 3]>`, each value `load × 100` - and the CLI renders it back to `x.xx`.
:::

## Errors

Two layers, by design:

- **`FrameError`** - pure framing error, `Clone + Eq`. Variants: `TooLarge { size, max }` and `PayloadOverflowsLengthPrefix { size }`.
- **`IpcError`** - top-level, wraps framing and serde errors. Variants: `Encode`, `Decode`, `Frame(FrameError)`, `UnexpectedEof { bytes }`, and `Io { kind: std::io::ErrorKind }`. It is **not** `Clone`/`Eq` because `serde_json::Error` is not.

For GUI/Tauri command returns that need `Clone + Eq`, call `IpcError::kind()` to get the shadow enum `IpcErrorKind`, which mirrors every variant (dropping the non-cloneable `serde_json::Error` detail) and carries `std::io::ErrorKind` (which is `Copy + Eq`). A `frame_error_to_kind_is_exhaustive` test enforces that every `FrameError` variant has a paired `IpcErrorKind`, with a `FrameOther { description }` catch-all guarding against drift.

EOF is split from framing: the pure codec never synthesises EOF. The transport `read_frame` returns `Ok(None)` on a clean EOF with an empty buffer, and `Err(IpcError::UnexpectedEof { bytes })` on EOF mid-frame. I/O failures map to `IpcError::Io { kind }`, preserving only the OS error category.

## Transport helpers

With `--features transport`, three async helpers in `transport.rs` glue the codec to a `tokio` stream:

```rust
pub async fn write_message<W, T>(writer: &mut W, value: &T, max: usize) -> Result<(), IpcError>;
pub async fn read_frame<R>(reader: &mut R, decoder: &mut FrameDecoder)
    -> Result<Option<Vec<u8>>, IpcError>;
pub async fn read_message<R, T>(reader: &mut R, decoder: &mut FrameDecoder)
    -> Result<Option<T>, IpcError>;
```

`read_frame` loops: try `decoder.next_frame()`, and on `None` read another chunk (a 4 KiB scratch buffer) into the decoder. It returns the raw payload so a caller can inspect the `type` tag before fully decoding. `read_message` is `read_frame` then `decode_message`. The daemon's per-client loop drives exactly these (`bin/yerdd/src/ipc_server.rs`):

```rust
let mut decoder = FrameDecoder::new();
loop {
    let req = match read_message::<_, Request>(&mut reader, &mut decoder).await {
        Ok(Some(r)) => r,
        Ok(None)    => return,            // clean EOF - client hung up
        Err(_)      => return,            // decode/EOF error - close quietly
    };
    let resp = dispatch(req, &state).await;
    write_message(&mut writer, &resp, DEFAULT_MAX_FRAME).await?;
}
```

A decode error (unknown `type`, unknown `ErrorCode`, malformed JSON) closes the connection quietly at debug level - that is the common signature of a mismatched-version client.

## Wire-stability tests

The JSON shapes are the **published contract**, pinned three ways so a rename or reshape cannot land silently:

1. **`tests/wire_stability.rs`** asserts byte-exact JSON for *every* `Request`, `Response`, `ErrorCode`, and status sub-type, e.g.

   ```rust
   assert_eq!(serde_json::to_string(&Request::SetPhp {
       name: "foo".into(), version: PhpVersion::new(8, 3),
   }).unwrap(), r#"{"type":"set_php","name":"foo","version":"8.3"}"#);
   ```

   It also pins additive back-compat: `Response::Info` decodes legacy daemons that omit `http_port`/`https_port` (they default to 0), and `Response::PhpVersions` skips an empty `updates`/`settings` on the wire so the bytes match the pre-field shape.

2. **Inline `variant_name_pinning` modules** in `request.rs` and `response.rs` contain exhaustive `match` arms over the (in-crate, so matchable despite `#[non_exhaustive]`) enums. A renamed Rust variant fails to compile there - integration tests can't catch this across the crate boundary.

3. **A CI grep gate** forbids per-field `#[serde(rename = "...")]` in `crates/yerd-ipc/src/`. Casing is owned entirely by `rename_all = "snake_case"`. Pairing the no-rename rule with the byte pins means a Rust rename trips *both* the wire pin (changed JSON) and the compile-time match - you cannot mask a rename with a `serde` attribute.

`tests/roundtrip.rs` additionally pins an `encode_message` ∘ `decode_message` identity plus the deliberate envelope/inner asymmetry: the outer `Request`/`Response` envelope **accepts** unknown JSON fields (so additive changes stay compatible), while an inner `Site` is **strict** (`deny_unknown_fields`). So `{"type":"ping","__extra":42}` decodes as `Request::Ping`, but an unknown field on a `Site` inside `Response::Sites` is rejected.

## Contract rules

The invariants that keep the protocol forward-compatible:

1. **Additive only.** Add variants and fields; never rename or remove a variant, field, or `ErrorCode`. `#[non_exhaustive]` keeps additions out of the breaking-change category.
2. **No per-field serde renames.** Let `rename_all` handle casing; the grep gate enforces it.
3. **Expand `ErrorCode`, don't overload `Internal`.** New typed failure categories get their own variant rather than collapsing into the catch-all.
4. **Fail closed.** Unknown `type` tags and unknown `ErrorCode`s surface as `IpcError::Decode`, not a silent `Unknown` downgrade.
5. **Versioning.** `PROTOCOL_VERSION` (currently `1`) is exposed but is effectively dead until a `Hello`/`Welcome` handshake lands; bump it only alongside that handshake.

   ```rust
   /// The current IPC protocol version. Bump on any breaking change;
   /// add a handshake before doing so.
   pub const PROTOCOL_VERSION: u32 = 1;
   ```

   Until then, a newer client talking to an older daemon is detected only when an unknown tag arrives mid-conversation - adding the handshake is the roadmap for proper version negotiation.

## See also

- [yerd-ipc crate reference](./crates/yerd-ipc)
- [The Daemon](../guide/daemon) and [yerdd internals](./binaries/yerdd)
- [yerd CLI internals](./binaries/yerd) · [Desktop App Internals](./gui)
- [Cross-Platform Model](./cross-platform) · [Architecture](./architecture)
