# yerd-proxy

`yerd-proxy` is the hand-rolled reverse proxy that terminates `*.test` HTTP/HTTPS traffic and forwards each routed request to its site backend. It is built directly on **hyper** (HTTP/1.1) and **tokio-rustls** (TLS termination) - there is no Caddy, no nginx, no embedded web server. The crate owns the accept loops, TLS handshake, request routing, the FastCGI client that talks to PHP-FPM, and the HTTP/1.1 client that talks to FrankenPHP workers.

Its source description is concise:

> HTTP/HTTPS reverse proxy for Yerd's `*.test` traffic.

The crate is deliberately decoupled from the rest of the workspace. It depends only on [`yerd-core`](./yerd-core) (for the `Site` / `SiteRouter` types) plus the async/HTTP/TLS stack. It does **not** depend on [`yerd-tls`](./yerd-tls) or [`yerd-php`](./yerd-php) - those couplings are inverted through two trait seams (`CertStore`, `BackendResolver`) injected by the daemon. See the [Crates Overview](../crates) for how it sits in the dependency graph and [The Daemon](../../guide/daemon) for the runtime that drives it.

::: info `#![forbid(unsafe_code)]`
The whole crate is `#![forbid(unsafe_code)]`. A compile-time guard in `lib.rs` also asserts `ProxyError: Send + Sync + 'static` so the error can cross hyper service boundaries and `tokio::spawn` sites cleanly.
:::

## Module map

```
crates/yerd-proxy/src/
├── lib.rs           # re-exports + Send/Sync compile guard
├── backend.rs       # Backend enum - where a routed request is forwarded
├── error.rs         # ProxyError
├── traits.rs        # CertStore + BackendResolver (daemon-injected seams)
├── tls.rs           # rustls ServerConfig build + SNI cert resolution
├── server.rs        # ProxyServer::serve - accept loops, dispatch
├── pure/            # synchronous, runtime-free, I/O-free helpers
│   ├── mod.rs
│   ├── cgi_params.rs   # build the CGI/1.1 param list for FastCGI
│   ├── fcgi_codec.rs   # FastCGI record framing (encode/decode)
│   └── redirect.rs     # HTTP → HTTPS redirect URI builder
└── forward/         # async per-backend forwarding I/O
    ├── mod.rs          # BoxBody + body helpers
    ├── fcgi.rs         # FastCGI forwarder (PHP-FPM)
    ├── http.rs         # plain HTTP/1.1 forwarder (FrankenPHP)
    └── upgrade.rs      # Connection: Upgrade tunnel (WebSocket etc.)
```

The split between `pure/` and `forward/` is the central design seam: **`pure/` is synchronous, allocation-only, I/O-free, and exhaustively table-tested**; `forward/` owns the actual socket reads and writes and the tokio runtime.

## The `pure/` layer

Everything in `pure/` is deterministic and unit-testable without a runtime, sockets, or a backend. This is where the fiddly protocol logic lives.

### `fcgi_codec` - FastCGI record framing

A from-scratch FastCGI codec. It does **encode/decode of records only** - the forwarder owns the socket. Constants pin the protocol shape:

```rust
pub const FCGI_VERSION: u8 = 1;
pub const FCGI_RESPONDER: u16 = 1;
pub const FCGI_MAX_PAYLOAD: usize = 65_535; // content_length is a u16
pub const FCGI_REQUEST_COMPLETE: u8 = 0;
```

The 8-byte record header round-trips through `Header::encode`/`Header::decode`:

```rust
pub struct Header {
    pub version: u8,
    pub record_type: RecordType,
    pub request_id: u16,
    pub content_length: u16,
    pub padding_length: u8,
}
```

`RecordType` is a `#[repr(u8)]` enum covering `BeginRequest` (1) through `UnknownType` (11), with `from_u8` for decode. Name/value pairs use FastCGI's length-prefix scheme via `encode_name_value`: lengths `<= 127` take one byte; longer lengths take four bytes with the high bit set on the first. `encode_begin_request_body(role, keep_conn)` produces the 8-byte BEGIN_REQUEST body (`role` big-endian, then a `FCGI_KEEP_CONN` flag byte). `EndRequest::decode` pulls the `app_status` (u32) and `protocol_status` (u8) out of the END_REQUEST body.

Decode is strict: `Header::decode` returns `FcgiError::BadVersion` when the version byte is not 1, `FcgiError::Short` when the slice is not exactly 8 bytes, and `FcgiError::UnknownRecordType` for unknown type bytes. The tests pin the wire layout exactly - e.g. encoding a 200-byte name yields a four-byte length of `[0x80, 0x00, 0x00, 0xC8]` (`200 | 0x80000000`).

### `cgi_params` - building the CGI/1.1 variable list

`build_params` turns an HTTP request into the `Vec<(Vec<u8>, Vec<u8>)>` of CGI variables sent in FastCGI `PARAMS` records:

```rust
pub fn build_params(
    method: &str,
    path_and_query: &str,
    headers: &http::HeaderMap,
    document_root: &Path,
    https: bool,
    remote_addr: SocketAddr,
    server_addr: SocketAddr,
) -> Vec<(Vec<u8>, Vec<u8>)>
```

::: warning MVP routing policy: "everything to index.php"
The current policy is Caddy-style front-controller routing - every request is mapped to the served root's `index.php`, regardless of the requested path:

- `SCRIPT_FILENAME = document_root / "index.php"`
- `SCRIPT_NAME     = "/index.php"`
- `PATH_INFO       = <original path>`
- `REQUEST_URI     = <original path_and_query>`

It does **not** yet serve static files directly or resolve arbitrary `.php` scripts on disk. That is a known limitation of the MVP forwarder.
:::

::: info `document_root` here is the site's *served web root*
The `document_root` parameter is the directory actually served, which is the site's web root - not necessarily its project root. The daemon passes `site.served_root()` (e.g. `<project>/public` for Laravel), so `SCRIPT_FILENAME` / `DOCUMENT_ROOT` resolve under the framework's front-controller directory. `build_params` itself is web-root-agnostic - it just joins `index.php` onto whatever root it's given. See [Sites → Web root](../../guide/sites#web-root-the-served-directory).
:::

Beyond the script vars, `build_params` emits the standard CGI/1.1 set (`GATEWAY_INTERFACE`, `SERVER_PROTOCOL`, `REQUEST_METHOD`, `QUERY_STRING`, `DOCUMENT_ROOT`, `REMOTE_ADDR`/`REMOTE_PORT`, `SERVER_ADDR`/`SERVER_PORT`, `SERVER_SOFTWARE = yerd`). `HTTPS=on` is added only when the request arrived on the TLS listener. `Host` is surfaced as both `SERVER_NAME` and `HTTP_HOST`; `Content-Type` and `Content-Length` are emitted un-prefixed (FPM expects them that way). Every other header is translated to the generic `HTTP_*` form (uppercased, `-` → `_`), with `Host`/`Content-Type`/`Content-Length` explicitly skipped so they are not double-emitted.

### `redirect` - HTTP → HTTPS upgrade URI

`build_redirect_uri(host, path_and_query, https_port)` constructs the `Location` for the permanent redirect used when a secure site is hit on the plain-HTTP listener:

```rust
pub fn build_redirect_uri(host: &str, path_and_query: &str, https_port: u16) -> String
```

It strips any inbound port from `host` (handling both `host:80` and bracketed IPv6 `[::1]:80`), lowercases the host, defaults an empty path to `/`, and appends `:port` only when the HTTPS port is not 443. The `strip_port` helper is careful about IPv6: a bracketed literal keeps everything up to and including the `]`, and a plain host is only split when it contains exactly one colon (so an unbracketed IPv6 address is left intact). All of this is exercised by a table test (`build_table`) covering `app.test:80`, `[::1]:80`, `[2001:db8::1]:80`, and the 443-vs-8443 port cases.

## The `Backend` enum

`Backend` (in `backend.rs`) is the single description of where a routed request goes:

```rust
#[non_exhaustive]
pub enum Backend {
    /// FastCGI over a Unix domain socket. Unix-only.
    PhpFpm { socket: PathBuf },
    /// FastCGI over TCP loopback. Required on Windows; allowed elsewhere.
    PhpFpmTcp { addr: SocketAddr },
    /// Plain HTTP/1.1 to a FrankenPHP worker.
    FrankenPhp { addr: SocketAddr },
}
```

Its `Display` impl produces the stable labels used in logs and `ProxyError`: `fpm-unix:<path>`, `fpm-tcp:<addr>`, `franken:<addr>`.

Crucially, **`From<yerd_php::Listen>` is intentionally not implemented**. The daemon's `BackendResolver` does that translation, keeping `yerd-proxy` free of any `yerd-php` dependency.

::: info FrankenPHP is wired in the proxy but not yet driven
The `FrankenPhp` variant and its HTTP/upgrade forwarders are fully implemented in this crate, but the daemon's resolver currently produces PHP-FPM backends. Treat the FrankenPHP path as forward-looking plumbing rather than a user-facing feature today.
:::

## Trait seams: `CertStore` and `BackendResolver`

These two traits (`traits.rs`) are how the daemon injects behaviour without `yerd-proxy` depending on `yerd-tls` or `yerd-php`.

```rust
pub trait CertStore: std::fmt::Debug + Send + Sync + 'static {
    fn certified_key(&self, sni_host: &str) -> Option<Arc<rustls::sign::CertifiedKey>>;
}

#[async_trait]
pub trait BackendResolver: Send + Sync + 'static {
    async fn backend_for(&self, site: &yerd_core::Site) -> Result<Backend, ProxyError>;
}
```

- **`CertStore` is synchronous** because rustls's `ResolvesServerCert::resolve` is synchronous - it is called inside the TLS handshake. The daemon's impl is expected to hold the active cert material in an in-memory map and refresh it out-of-band. See [HTTPS & Certificates](../../guide/https).
- **`BackendResolver` is async** and consulted once per request, mapping the routed `&Site` to a concrete `Backend`. The daemon's impl typically calls `yerd_php::PhpManager::ensure(site.php())` and translates the returned `Listen` into a `Backend`. The implementer note in the source is load-bearing: copy out the `Site` fields you need before any `.await`, so the per-request closure doesn't hold a router guard across an await point.

Foreign errors (e.g. `PhpError`) are boxed into `ProxyError::BackendResolver { host, source }`, so the proxy never names `yerd-php` in its type signatures.

## TLS and per-SNI cert selection

`tls.rs` wires `CertStore` into rustls. `build_server_config` constructs a `rustls::ServerConfig` with no client auth and an `SniResolver` as the cert resolver:

```rust
pub fn build_server_config<C: CertStore>(store: Arc<C>) -> Arc<ServerConfig> {
    init_crypto_once();
    let resolver = Arc::new(SniResolver::new(store));
    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_cert_resolver(resolver);
    Arc::new(config)
}
```

`SniResolver::resolve` reads the SNI host from the `ClientHello` and delegates to `CertStore::certified_key`. **A miss is a hard refusal**: it returns `None` (logged at debug as "SNI miss - dropping connection"), which aborts the handshake rather than presenting a default certificate.

`init_crypto_once` installs the ring `CryptoProvider` as the process default exactly once (via `OnceLock`). This is required because the workspace pins rustls 0.23 with no preinstalled global provider; without it, the first `ServerConfig::builder()` call would panic. The function is idempotent and tolerates another provider already being installed (multi-process tests, multi-binary daemons) - it only needs *some* provider in place. `ProxyServer::serve` calls it before binding anything.

## Binding: never bind privileged ports directly

`yerd-proxy` does **not** bind ports itself. The caller binds via [`yerd-platform`](./yerd-platform)'s `PortBinder` and hands the listeners in already-bound. This keeps all privileged-socket logic - and the rootless fallback - out of the proxy and in the platform layer. See [Elevation & Privileges](../../guide/elevation).

`HttpsBinding` documents the contract precisely:

```rust
pub struct HttpsBinding<C: CertStore> {
    /// The bound TCP listener (caller obtained from `PortBinder::bind_pair`
    /// and converted via `tokio::net::TcpListener::from_std`).
    pub listener: TcpListener,
    /// Public port the HTTP→HTTPS redirect should target - not
    /// necessarily what `listener.local_addr()` reports.
    pub public_port: u16,
    /// Cert lookup. Arc-wrapped so the SNI resolver can clone cheaply.
    pub cert_store: Arc<C>,
}
```

`public_port` is separate from the listener's local port on purpose: in rootless mode the daemon may `bind_pair((80, 443), (8080, 8443))` and end up listening on 8443, but the redirect target must reflect the port clients actually use. The daemon's `bind_pair` atomically binds the desired HTTP/HTTPS pair, falling back to `(8080, 8443)` when `80`/`443` require elevation, then converts each `std` listener with `TcpListener::from_std` before passing it in.

## The server: `ProxyServer::serve`

`server.rs` is the runtime entry point:

```rust
pub async fn serve<R, C, S>(
    http_listener: TcpListener,
    https: Option<HttpsBinding<C>>,
    router: SharedRouter,
    backend_resolver: Arc<R>,
    shutdown: S,
) -> Result<(), ProxyError>
where
    R: BackendResolver,
    C: CertStore,
    S: Future<Output = ()> + Send + 'static,
```

`SharedRouter` is `Arc<tokio::sync::RwLock<yerd_core::SiteRouter>>`. Reads are brief: each request takes a read guard only long enough to `resolve(&host)` and clone the matched `Site` (cheap - small strings and `PathBuf`s), then drops the guard before any `.await`. The daemon is the only writer and swaps the whole router under a write guard when a site is parked/linked/unlinked or its PHP version changes.

### Lifecycle

1. `init_crypto_once()`.
2. A shutdown task awaits the `shutdown` future, then calls `notify_waiters()` on an internal `Notify`.
3. Two accept loops are spawned: one HTTP, one HTTPS (only if `https` is `Some`). Each loop is a `tokio::select! { biased; … }` that breaks on the notify and otherwise accepts connections. `biased` makes shutdown take priority over a ready accept.
4. Each accepted connection is handled in its own `tokio::spawn`. The HTTPS path first runs `TlsAcceptor::accept`; handshake failures are logged at debug and the connection is dropped.
5. Connections are served with `hyper::server::conn::http1::Builder::new().serve_connection(io, svc).with_upgrades()` - `with_upgrades()` is required for the WebSocket tunnel path.

On shutdown, accept loops stop immediately; in-flight requests run to hyper's default timeouts. `serve` returns once both accept tasks and the shutdown task have joined.

### Per-request dispatch

The hyper service is **infallible** - internal errors are logged and turned into a `500` so hyper's connection loop survives. `dispatch` does the real work, in order:

1. **Host header.** Missing or non-UTF-8 → `400 Bad Request` ("Missing or invalid Host header.").
2. **Route.** `router.resolve(&host)`; no match → `404 Not Found` ("No site matches this Host."). The matched `Site` is cloned and the guard dropped; the request is served from `site.served_root()` (the site's web root, e.g. `<project>/public`).
3. **HTTP → HTTPS redirect.** On the HTTP listener, if `site.secure()` is true and a `redirect_port` is set, return `301 Moved Permanently` with `Location` built by `build_redirect_uri`.
4. **Resolve backend** via `BackendResolver::backend_for(&site)`. Errors already in the connect/protocol/resolver family pass through; any other variant is wrapped in `ProxyError::BackendResolver { host, source }`.
5. **Upgrade dispatch.** If `upgrade::is_upgrade(headers)`, forward to `upgrade::forward` for `FrankenPhp`, or return `501 Not Implemented` for FastCGI backends (FastCGI cannot model a duplex byte stream).
6. **Normal dispatch.** `FrankenPhp` → `http::forward`; `PhpFpm`/`PhpFpmTcp` → `fcgi::forward`.

The `Listener::{Http, Https}` discriminator is threaded through so the redirect rule and the `HTTPS=on` CGI var both know which listener the connection arrived on.

## The `forward/` layer

All response bodies share one type so every path returns the same `Response`:

```rust
pub type BoxBody = http_body_util::combinators::BoxBody<bytes::Bytes, std::io::Error>;
```

with `empty_body()` (for 301/404/501/101) and `bytes_body(&'static [u8])` helpers.

### `fcgi` - the PHP-FPM forwarder

`fcgi::forward` drives the full FastCGI exchange against PHP-FPM:

1. **Connect.** `open_backend` opens a `UnixStream` for `PhpFpm { socket }` (Unix only - non-Unix returns `ErrorKind::Unsupported`) or a `TcpStream` for `PhpFpmTcp { addr }`. The two are unified behind a `BackendStream` enum that implements `AsyncRead`/`AsyncWrite`. (`FrankenPhp` reaching this path is a `#[cold]` "dispatch bug" error.)
2. **BEGIN_REQUEST** with `FCGI_RESPONDER`, `keep_conn = false`, `request_id = 1`.
3. **PARAMS** from `build_params`, chunked at `FCGI_MAX_PAYLOAD`, followed by a zero-length PARAMS terminator. The prelude is flushed before the body is drained.
4. **STDIN.** The request body is streamed frame-by-frame, chunked at `FCGI_MAX_PAYLOAD`, then a zero-length STDIN terminator. HTTP trailers are dropped (FastCGI cannot represent them).
5. **Read STDOUT/STDERR** until `END_REQUEST`. Each record's content and padding are read with `read_exact`; a `request_id != 1` yields `FcgiError::UnexpectedRequestId`. Any non-empty STDERR is logged at warn ("FPM stderr").
6. **Synthesise the response.** `parse_cgi_response` splits the CGI header block at the first `\r\n\r\n` or `\n\n`, translates `Status: NNN [Reason]` into the HTTP status (defaulting to `200 OK`), and copies the rest as the body. Headers that fail `HeaderName`/`HeaderValue` validation are silently skipped.

`upgrade_not_supported()` is the `501` response for upgrade attempts on a FastCGI backend.

### `http` - the FrankenPHP forwarder

`http::forward` connects to the FrankenPHP worker over TCP and uses **raw `hyper::client::conn::http1::handshake`** rather than `hyper-util`'s pooled `legacy` client - the comment notes the pooled client has historical upgrade gotchas and doesn't expose the upgraded socket cleanly. The driver connection runs in a detached task; the response body is re-boxed into `BoxBody` (mapping the hyper body error into `io::Error`).

### `upgrade` - the WebSocket/Upgrade tunnel

`upgrade::is_upgrade` detects `Connection: Upgrade` per RFC 9110 §7.8, handling comma-separated, case-insensitive tokens (`keep-alive, Upgrade`) and requiring the `Upgrade` header to be present.

`upgrade::forward` implements the hyper-1 upgrade dance:

1. Capture the client's upgrade future (`hyper::upgrade::on(&mut req)`) **before** moving the request upstream.
2. Open the backend connection with raw `http1::handshake().with_upgrades()`.
3. Rebuild the upstream request with an `Empty` body (bytes flow through the upgraded socket, not the body) and send it.
4. Capture the backend's upgrade future from the response.
5. Strip hop-by-hop headers (`strip_hop_by_hop` removes the fixed set - `connection`, `proxy-connection`, `keep-alive`, `te`, `transfer-encoding`, `trailer` - plus any tokens listed in `Connection:`, while preserving `Upgrade`, then re-inserts a fresh `Connection: upgrade`).
6. Return the `101` to the service so hyper flushes it to the client, then in a detached task `try_join!` both upgrade futures and `copy_bidirectional` the two `Upgraded` streams (each wrapped in `TokioIo`).

## Errors

`ProxyError` (`error.rs`) is `#[non_exhaustive]` and intentionally not `Clone`/`Eq` (it wraps `io::Error`, `hyper::Error`, `rustls::Error`, and a boxed `dyn Error`). Variants: `Accept`, `BackendResolver { host, source }`, `BackendConnect { backend, source }`, `BackendProtocol`, `Upgrade`, `Fcgi` (`#[from] FcgiError`), `Hyper`, and `Tls`. The daemon translates these to a stable code when crossing the IPC wire - see the [IPC Protocol](../../developer/ipc-protocol).

## Tests and invariants

- **`pure/` unit tests** pin the wire format and policy: `fcgi_codec` round-trips headers and verifies short/long name-value encoding; `cgi_params` asserts the Caddy-style mapping, the `HTTPS=on` rule, and `HTTP_*` translation; `redirect` runs the full host/port table.
- **`tests/integration_http.rs`** drives `ProxyServer::serve` against a fake FastCGI listener and a hyper client, asserting both the round-trip body (`hello`) and the captured CGI params (`REQUEST_METHOD`, `REQUEST_URI`, `SCRIPT_NAME`, `PATH_INFO`, `QUERY_STRING`, `SERVER_NAME`). It also covers the `404` (unknown host) and `400` (missing Host) paths.
- **`tests/integration_https.rs`** issues a real CA + leaf from [`yerd-tls`](./yerd-tls), serves over TLS with a single-host `CertStore`, and drives a rustls hyper client through the full SNI handshake to the fake backend.
- **`tests/no_runtime_deps.rs`** is a dependency-graph guard: it walks `cargo metadata` and asserts the runtime graph never pulls `anyhow`, the OpenSSL/native-tls family, `hyper-tls`, `tokio-native-tls`, or `webpki-roots`, and that `hyper`, `rustls`, `tokio`, and `time` each resolve to a single version. This keeps the proxy on a pure-Rust, rustls-only TLS stack.

## See also

- [yerd-tls](./yerd-tls) - the CA and leaf issuance that backs the daemon's `CertStore`.
- [yerd-php](./yerd-php) - the PHP-FPM supervisor whose `Listen` the daemon translates into a `Backend`.
- [yerd-platform](./yerd-platform) - `PortBinder`/`bind_pair` and the rootless port fallback.
- [yerdd (daemon)](../binaries/yerdd) - the binary that binds the ports, builds the `CertStore`/`BackendResolver`, and calls `ProxyServer::serve`.
- Source: [github.com/forjedio/yerd](https://github.com/forjedio/yerd)
