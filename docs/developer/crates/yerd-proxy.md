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
│   ├── try_files.rs    # static-file/directory-index candidate resolution + MIME map
│   └── redirect.rs     # HTTP → HTTPS redirect URI builder
└── forward/         # async per-backend forwarding I/O
    ├── mod.rs          # BoxBody + body helpers
    ├── static_file.rs  # serve a real static file, or a directory's index.html/.htm
    ├── script_file.rs  # resolve a real, on-disk PHP script to execute directly (WordPress-gated)
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

::: info Front-controller routing for dynamic requests
For requests that reach FastCGI, the policy is Caddy-style front-controller routing - the request is mapped to the served root's `index.php`:

- `SCRIPT_FILENAME = document_root / "index.php"`
- `SCRIPT_NAME     = "/index.php"`
- `PATH_INFO       = <original path>`
- `REQUEST_URI     = <original path_and_query>`

Static files are handled *before* this, by a `try_files`-style short-circuit (see [`try_files`](#try_files-static-file-resolution) and [`static_file`](#static_file-serving-real-files)): a request that resolves to a real, non-PHP file under the served root is returned directly. A directory-style request (trailing slash, including the site root) with no `index.php` falls back to that directory's `index.html`/`index.htm` next, so a plain static site works without a front controller at all. Only everything else falls through to FastCGI. Arbitrary on-disk `.php` scripts are still routed through `index.php` (and PHP source is never served as a static file, directly or via a directory index).
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

### `try_files` - static-file resolution {#try_files-static-file-resolution}

`try_files` decides, purely, whether a request *could* be a static file and what its safe relative path and MIME type would be. It does no I/O - the [`static_file`](#static_file-serving-real-files) forwarder does the actual stat/read.

- **`static_candidate(path)`** maps a URL path to a safe relative `PathBuf`, or `None` when the request must go to the front controller instead. It returns `None` for `/`, for a directory-style request (trailing slash), and for any traversal attempt. It percent-decodes the path and rejects encoded slashes and NUL bytes, so a decoded segment can never escape the served root.
- **`directory_candidate(path)`** is `static_candidate`'s counterpart for directory-index resolution: it maps a *directory-style* URL path (trailing slash, or the bare root `/`) to a safe relative directory `PathBuf`, or `None` for anything else. Same percent-decoding and traversal rules as `static_candidate` - the two intentionally partition every URL shape between them (a path never satisfies both).
- **`is_php_source(path)`** flags PHP source extensions (`php`, `phtml`, `php3`/`php4`/`php5`/`php7`, `phps`, `pht`) so they are *never* served as static bytes - they fall through to FastCGI.
- **`content_type_for(path)`** maps a file extension to a `Content-Type` for the response (a small MIME table, defaulting to `application/octet-stream`).

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

## Trait seams: `CertStore`, `BackendResolver`, `LoginTokenConsumer`

These three traits (`traits.rs`) are how the daemon injects behaviour without `yerd-proxy` depending on `yerd-tls`, `yerd-php`, or any concrete daemon state.

```rust
pub trait CertStore: std::fmt::Debug + Send + Sync + 'static {
    fn certified_key(&self, sni_host: &str) -> Option<Arc<rustls::sign::CertifiedKey>>;
}

#[async_trait]
pub trait BackendResolver: Send + Sync + 'static {
    async fn backend_for(&self, site: &yerd_core::Site) -> Result<Backend, ProxyError>;

    /// Whether `site` allows `script_file::resolve_script`'s direct-real-file-
    /// execution policy - defaults to `false`.
    async fn allows_direct_script_execution(&self, site: &yerd_core::Site) -> bool {
        false
    }
}

pub trait LoginTokenConsumer: Send + Sync + 'static {
    fn consume(&self, site: &str, token: &str) -> Option<String>;
}
```

- **`CertStore` is synchronous** because rustls's `ResolvesServerCert::resolve` is synchronous - it is called inside the TLS handshake. The daemon's impl is expected to hold the active cert material in an in-memory map and refresh it out-of-band. See [HTTPS & Certificates](../../guide/https).
- **`BackendResolver` is async** and consulted once per request, mapping the routed `&Site` to a concrete `Backend`. The daemon's impl typically calls `yerd_php::PhpManager::ensure(site.php())` and translates the returned `Listen` into a `Backend`. The implementer note in the source is load-bearing: copy out the `Site` fields you need before any `.await`, so the per-request closure doesn't hold a router guard across an await point.
- **`allows_direct_script_execution` is a default method** (defaulting to `false`) gating [`script_file::resolve_script`](#script_file-direct-script-execution) - a real, on-disk `.php` script under the served root is only directly URL-executable for sites the resolver opts in. WordPress needs this for its multiple front controllers (`wp-login.php`, `wp-admin/index.php`, ...); a framework with a single front controller (Laravel, plain PHP) does not, and leaving it on for those would make any stray script under the document root (a debug `phpinfo.php`, an old admin tool) directly URL-executable where it previously wasn't. The daemon's impl (`DaemonBackendResolver`, see [`yerdd`](../binaries/yerdd#backend-resolver-backend-resolver-rs)) checks its `wordpress_sites` cache.
- **`LoginTokenConsumer` is synchronous** and backs the one-click WordPress admin login flow - see [One-click WordPress admin login](#one-click-wordpress-admin-login) below. `consume` must check and invalidate atomically, so a token can never be consumed twice even under concurrent requests for the same token.

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
    /// necessarily what `listener.local_addr()` reports. Shared so the
    /// daemon can flip it live, without restarting the proxy.
    pub public_port: Arc<AtomicU16>,
    /// Cert lookup. Arc-wrapped so the SNI resolver can clone cheaply.
    pub cert_store: Arc<C>,
}
```

`public_port` is separate from the listener's local port on purpose: in rootless mode the daemon may `bind_pair((80, 443), (8080, 8443))` and end up listening on 8443, but the redirect target must reflect the port clients actually use. The daemon's `bind_pair` atomically binds the desired HTTP/HTTPS pair, falling back to `(8080, 8443)` when `80`/`443` require elevation, then converts each `std` listener with `TcpListener::from_std` before passing it in.

It's an `Arc<AtomicU16>` rather than a plain `u16` because the fallback story doesn't end at startup: on macOS, `yerd elevate ports` installs a `pf` redirect (`80`/`443` → the bound rootless pair) that goes live immediately, with no daemon restart - see [Elevation & Privileges](../../guide/elevation). If the redirect target stayed a fixed `u16` captured once in `ProxyServer::serve`, a browser would keep getting bounced to `https://site.test:8443` even after elevation made the plain `https://site.test` reachable. Instead, the daemon owns a shared cell (`DaemonState::redirect_https_port`, see [`yerdd`](../binaries/yerdd)) that a background prober flips between the rootless and well-known port as `yerd_platform::PortRedirector::is_active` changes, and `dispatch` loads the current value on every redirect it builds. `yerd-proxy` itself has no opinion on *why* the port changes - it just reads whatever the cell holds at request time.

## The server: `ProxyServer::serve`

`server.rs` is the runtime entry point:

```rust
pub async fn serve<R, C, S, L>(
    http_listener: TcpListener,
    https: Option<HttpsBinding<C>>,
    router: SharedRouter,
    backend_resolver: Arc<R>,
    login_tokens: Arc<L>,
    login_prepend_script: Option<PathBuf>,
    shutdown: S,
) -> Result<(), ProxyError>
where
    R: BackendResolver,
    C: CertStore,
    S: Future<Output = ()> + Send + 'static,
    L: LoginTokenConsumer,
```

`login_tokens` and `login_prepend_script` back the one-click WordPress admin login flow (see [below](#one-click-wordpress-admin-login)) - `login_prepend_script` is `None` when the daemon couldn't write its auto-login bootstrap script at startup, in which case a presented token is simply never consumed and the request falls through unauthenticated.

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
2. **Route** via `resolve_request`, producing a `Routed::Site { site, unbound }` or a ready-made `Routed::Respond`. Normal `Host` resolution (`router.resolve(&host)`) is tried first; on a miss, if the host is loopback, an "unbound" (resolver-off) fallback applies - a pinned-site cookie, an `X-Yerd-Site` header, or a same-origin site picker page, none of which are covered here. Anything else on a miss is `404 Not Found` ("No site matches this Host."). The matched `Site` is cloned and the router guard dropped before any further `.await`; the request is served from `site.served_root()` (the site's web root, e.g. `<project>/public`).
3. **HTTP → HTTPS redirect.** On the HTTP listener, unless the request resolved via the unbound fallback, if `site.secure()` is true and a `redirect_port` is set, return `301 Moved Permanently` with `Location` built by `build_redirect_uri`. This runs **before** the one-click login token is ever looked at (see below), so a secure site's token is never burned by the 301 itself.
4. **Resolve backend** via `BackendResolver::backend_for(&site)`. Errors already in the connect/protocol/resolver family pass through; any other variant is wrapped in `ProxyError::BackendResolver { host, source }`.
5. **Upgrade dispatch.** If `upgrade::is_upgrade(headers)`, forward to `upgrade::forward` for `FrankenPhp`, or return `501 Not Implemented` for FastCGI backends (FastCGI cannot model a duplex byte stream).
6. **Normal dispatch.** `FrankenPhp` → `http::forward` directly. `PhpFpm`/`PhpFpmTcp` → `serve_php_fpm`, which runs four steps in order:
   1. **One-click login token.** `consume_login_token_if_present` (see [below](#one-click-wordpress-admin-login)).
   2. **Static-file short-circuit.** `static_file::try_serve`: a GET/HEAD request that resolves to a real, non-PHP file under the served root - allowing symlinks that resolve anywhere within the site's `document_root`, not just the served subdirectory - is returned directly with a guessed `Content-Type`. A candidate that resolves outside `document_root` gets an explicit `403 Forbidden` from yerd-proxy instead of falling through.
   3. **Directory-index short-circuit.** If the static short-circuit didn't match, `static_file::try_serve_index` is tried next - a GET/HEAD directory-style request (trailing slash, including the site root) with no `index.php` in that directory serves its `index.html`/`index.htm` directly, so plain static sites work with no PHP front controller at all. Same `document_root` containment and `403` behavior as `try_serve`.
   4. **Real-script resolution**, gated by `allows_direct_script_execution` - see [script_file: direct script execution](#script_file-direct-script-execution) - then `fcgi::forward` regardless of what was resolved (a script path, or `None` to fall back to the site root's `index.php`).

`FrankenPhp` serves its own static files and has no equivalent script-resolution step, so steps 6.2-6.4 only apply to the FastCGI backends. The `Listener::{Http, Https}` discriminator is threaded through so the redirect rule and the `HTTPS=on` CGI var both know which listener the connection arrived on.

## One-click WordPress admin login {#one-click-wordpress-admin-login}

`consume_login_token_if_present` (`server.rs`) checks a request for the one-click WordPress admin login token the daemon minted via `Request::MintWordpressLoginToken` (see [`yerdd`](../binaries/yerdd)'s `wordpress_login` module):

```rust
const LOGIN_TOKEN_PARAM: &str = "yerd_login_token";

fn consume_login_token_if_present<B, L: LoginTokenConsumer>(
    req: &mut Request<B>,
    site_name: &str,
    login_tokens: &L,
) -> Option<String>
```

- Only ever considered on a `/wp-admin` path prefix - every other request skips the check entirely without touching `login_tokens`.
- Consumes the token via `LoginTokenConsumer::consume(site_name, token)`, which must be atomic (check-and-invalidate in one step) so concurrent requests can't both succeed against the same token.
- On success, strips `yerd_login_token` from the forwarded URI (via `pure::query::strip_param`) so it never reaches PHP or request logging, and returns `Some(target_user)` (`""` meaning no preference - the caller falls back to the earliest-created administrator).
- **Ordering is load-bearing**: `serve_php_fpm` calls this *after* `dispatch`'s HTTP→HTTPS redirect check has already run (step 3 above), so a secure site's token is never burned by the 301 itself - a browser presenting the token over plain HTTP gets redirected first, with the token intact in the `Location`, and only consumes it on the follow-up HTTPS request.

On success, `serve_php_fpm` builds a `cgi_params::AutoLoginParams { prepend_script, target_user }` and passes it through to `fcgi::forward`, which adds it as a per-request `auto_prepend_file` FastCGI param (plus a `YERD_LOGIN_USER` param) - so the injected bootstrap script only ever loads for this one already-token-validated request, never for an ordinary one. `auto_login` is only built when `login_prepend_script` is `Some` *and* a token was just consumed - the daemon's own `mint_wordpress_login_token` handler already refuses to mint a token at all when the prepend script is unavailable (see [`yerdd`](../binaries/yerdd)), so in practice the proxy never sees a valid token to consume while `login_prepend_script` is `None`.

## The `forward/` layer

All response bodies share one type so every path returns the same `Response`:

```rust
pub type BoxBody = http_body_util::combinators::BoxBody<bytes::Bytes, std::io::Error>;
```

with `empty_body()` (for 301/404/501/101) and `bytes_body(&'static [u8])` helpers.

### `static_file` - serving real files {#static_file-serving-real-files}

Both lookup functions return a `StaticOutcome`: `Served(Response)`, `NotFound` (fall through to the front controller, same as a bare `None` before this type existed), or `SymlinkEscape { requested_path, resolved, allowed_root }` (the candidate resolved via symlink to somewhere outside the site's `document_root` - the caller turns this into a `403` via `symlink_escape_response`, never a silent fallthrough). The `403` body names only `requested_path`; `resolved` and `allowed_root` go to a `tracing::warn!` (target `yerd_proxy::static_file`) instead, since a site can be exposed beyond loopback via `yerd-tunnel` and absolute local paths shouldn't reach a remote client.

`static_file::try_serve` is the static short-circuit for the FastCGI backends. Given the served root, the site's `document_root`, and the request, it:

1. asks [`try_files::static_candidate`](#try_files-static-file-resolution) for a safe relative path (`NotFound` for `/`, directory requests, traversal, or a non-GET/HEAD method);
2. refuses PHP source (`is_php_source`) so a `.php` file is never returned as bytes;
3. joins the candidate onto the served root, **canonicalises**, and verifies the result is still inside the site's `document_root` - not just the served subdirectory, so e.g. Laravel's `public/storage -> ../storage/app/public` symlink is served normally even though it points outside `public/`. A candidate that canonicalises fine but lands outside `document_root` entirely is a `SymlinkEscape`; a candidate that simply doesn't exist is `NotFound`, unchanged;
4. on a hit, reads the file and returns `200 OK` with the `Content-Type` from `content_type_for` and the `Server: <PROXY_SERVER_ID>` header (a `HEAD` returns an empty body).

`static_file::try_serve_index` is the directory-index counterpart, tried next when `try_serve` misses. Given the same served root, `document_root`, and the request, it:

1. asks [`try_files::directory_candidate`](#try_files-static-file-resolution) for a safe relative directory path (`NotFound` for anything that isn't a directory-style request, or a non-GET/HEAD method);
2. joins the candidate onto the served root, canonicalises, and verifies it's still inside `document_root` and is actually a directory - an escaped directory candidate is reported immediately, since there's no other candidate to fall back to;
3. defers to the front controller (`NotFound`) if that directory contains `index.php` - the front controller always wins when present;
4. otherwise probes `index.html`, then `index.htm`: each candidate is joined onto the (already-canonical) directory and **re-canonicalised in its own right** against `document_root` before being served. If one candidate escapes but the other is servable, the servable one still wins - a `SymlinkEscape` is only reported once every candidate has been tried and none served.
5. on a hit, serves the file exactly like `try_serve` (same `Content-Type` lookup, `HEAD` handling, headers).

A `NotFound` result here means no index file exists (or `index.php` won) and the request falls through to `fcgi::forward` exactly as it did before this short-circuit existed.

### `script_file` - direct script execution {#script_file-direct-script-execution}

`script_file::resolve_script` extends `cgi_params`'s "everything to `index.php`" front-controller policy with the `try_files $uri $uri/index.php` half of the classic WordPress/nginx policy: a real, more specific script wins over the site root's `index.php` when one exists.

```rust
pub async fn resolve_script(
    uri_path: &str,
    served_root: &Path,
    allowed_root: &Path,
) -> Option<PathBuf>
```

- Only called at all when `BackendResolver::allows_direct_script_execution(site)` returns `true` (see [`resolve_script_if_allowed`](#per-request-dispatch), which skips the filesystem check entirely otherwise) - a Laravel or plain-PHP site never reaches this function.
- Checks, in order: an exact non-directory match (`/wp-login.php` → `wp-login.php`), then - for a directory-style request - that directory's own index (`/wp-admin/` → `wp-admin/index.php`).
- Unlike `static_file`, this applies to **every HTTP method**, not just GET/HEAD - a real script like `wp-login.php` handles POST too. It never reads or serves file *content*, only decides which path FastCGI should be told to execute.
- Same canonicalise-and-check-containment discipline as `static_file`: a symlinked script that resolves outside `allowed_root` (`document_root`) is treated as not found (falls back to the root `index.php` policy) rather than handed to FastCGI.
- `None` (fall back to the site root's `index.php`, today's behavior for every framework with only one front controller) whenever there's no real, on-disk, non-directory `.php` file at the resolved path.

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
- **`tests/integration_http.rs`** drives `ProxyServer::serve` against a fake FastCGI listener and a hyper client, asserting both the round-trip body (`hello`) and the captured CGI params (`REQUEST_METHOD`, `REQUEST_URI`, `SCRIPT_NAME`, `PATH_INFO`, `QUERY_STRING`, `SERVER_NAME`). It also covers the `404` (unknown host) and `400` (missing Host) paths, plain static-file serving, and the directory-index fallback end to end: `index.html`/`index.htm` served when there's no `index.php`, `index.php` winning when both are present (asserted via the captured `SCRIPT_NAME`, not just the response body), a directory with no index of any kind and a nonexistent directory both still falling through to FastCGI, `HEAD` returning an empty body, and a symlinked `index.html` escaping the served root being refused rather than served.
- **`tests/integration_https.rs`** issues a real CA + leaf from [`yerd-tls`](./yerd-tls), serves over TLS with a single-host `CertStore`, and drives a rustls hyper client through the full SNI handshake to the fake backend.
- **`tests/no_runtime_deps.rs`** is a dependency-graph guard: it walks `cargo metadata` and asserts the runtime graph never pulls `anyhow`, the OpenSSL/native-tls family, `hyper-tls`, `tokio-native-tls`, or `webpki-roots`, and that `hyper`, `rustls`, `tokio`, and `time` each resolve to a single version. This keeps the proxy on a pure-Rust, rustls-only TLS stack.

## See also

- [yerd-tls](./yerd-tls) - the CA and leaf issuance that backs the daemon's `CertStore`.
- [yerd-php](./yerd-php) - the PHP-FPM supervisor whose `Listen` the daemon translates into a `Backend`.
- [yerd-platform](./yerd-platform) - `PortBinder`/`bind_pair` and the rootless port fallback.
- [yerdd (daemon)](../binaries/yerdd) - the binary that binds the ports, builds the `CertStore`/`BackendResolver`, and calls `ProxyServer::serve`.
- Source: [github.com/forjedio/yerd](https://github.com/forjedio/yerd)
