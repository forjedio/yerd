---
applyTo: "crates/yerd-proxy/**/*.rs"
---

# yerd-proxy — reverse proxy

Terminates HTTP/1.1 and HTTP/2 (TLS via rustls), routes by `Host` using
`yerd-core`, and forwards to the per-site backend. Hand-rolled on
`hyper` + `hyper-util` + `tokio-rustls` — deliberately small.

**Layer split:** `pure/` (`cgi_params`, `fcgi_codec`, `redirect`) is pure and
table-tested; `server.rs`, `tls.rs`, and `forward/` (`fcgi`, `http`, `upgrade`)
are the async edge. Side effects (port binding, cert lookup) go through
`traits.rs`.

## Owns

- Listening on `80`/`443` (or `8080`/`8443`) via a `PortBinder` from
  `yerd-platform` — never bind privileged ports directly.
- Per-SNI leaf-cert selection from an in-memory cert store (a `CertStore` trait).
- Forwarding: **FastCGI** to PHP-FPM (Unix socket on macOS/Linux, TCP loopback
  on Windows) or HTTP to a FrankenPHP worker; WebSocket upgrade pass-through;
  body streaming; HTTP/2.

## Must not

- Spawn or supervise PHP — that is `yerd-php`.
- Generate certificates — that is `yerd-tls` (this crate consumes cert types).
- Own DNS, or bind privileged ports without the `PortBinder` abstraction.

## Conventions

- Routing decisions belong in `yerd-core`; backend selection and the hand-rolled
  FastCGI record framing are pure and unit-tested here (`pure/fcgi_codec`,
  `pure/cgi_params`). Keep encoding logic out of the async forwarders.
- Preserve streaming and upgrade behaviour — do not buffer whole bodies or drop
  WebSocket upgrades when refactoring.

## Tests / invariants

- `tests/integration_http.rs` / `tests/integration_https.rs` — a request reaches
  the right backend; a WebSocket upgrade survives.
- `tests/no_runtime_deps.rs` — no OpenSSL/native-tls; rustls only.

## Review checklist

- [ ] Ports bound via `PortBinder`, certs fetched via the `CertStore` trait.
- [ ] FastCGI/CGI-param/redirect logic stays in `pure/` with table tests.
- [ ] Streaming + WebSocket upgrade preserved.
- [ ] rustls only; no OpenSSL in the graph.
