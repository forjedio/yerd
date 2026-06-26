//! `ProxyServer::serve` — accept loops + per-connection hyper service.

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;

use http::header::{
    ACCEPT, CACHE_CONTROL, CONTENT_TYPE, COOKIE, HOST, LOCATION, SERVER, SET_COOKIE,
};
use http::{HeaderValue, Method, StatusCode};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tokio_rustls::TlsAcceptor;
use yerd_core::{Site, SiteKind, SiteRouter};

use crate::backend::Backend;
use crate::error::ProxyError;
use crate::forward::{
    bytes_body, empty_body, fcgi, http as http_fwd, owned_bytes_body, static_file, upgrade, BoxBody,
};
use crate::pure::redirect::build_redirect_uri;
use crate::pure::unbound::{self, PickerSite};
use crate::tls::build_server_config;
use crate::traits::{BackendResolver, CertStore};

/// Router shared between the proxy's request path (read) and the daemon's
/// mutation path (write-replace). Reads are brief and uncontended — each
/// request takes a read guard only long enough to resolve + clone its
/// [`yerd_core::Site`]; the daemon swaps the whole router under a write guard
/// when a site is parked/linked/unlinked or its PHP version changes.
pub type SharedRouter = Arc<tokio::sync::RwLock<yerd_core::SiteRouter>>;

/// Discriminator threaded into each per-request service so the redirect
/// rule knows whether the connection arrived on the HTTP or HTTPS
/// listener.
#[derive(Clone, Copy, Debug)]
enum Listener {
    Http,
    Https,
}

/// HTTPS-related parameters for [`ProxyServer::serve`].
pub struct HttpsBinding<C: CertStore> {
    /// The bound TCP listener (caller obtained from `PortBinder::bind_pair`
    /// and converted via `tokio::net::TcpListener::from_std`).
    pub listener: TcpListener,
    /// Public port the HTTP→HTTPS redirect should target — not
    /// necessarily what `listener.local_addr()` reports (rootless
    /// mode may bind 8443 but the redirect should still go to 8443).
    pub public_port: u16,
    /// Cert lookup. Arc-wrapped so the SNI resolver can clone cheaply.
    pub cert_store: Arc<C>,
}

/// Top-level proxy entry point.
pub struct ProxyServer;

impl ProxyServer {
    /// Run until `shutdown` resolves.
    ///
    /// Spawns one task per accepted connection; cancels them on shutdown
    /// via an internal `Notify`. In-flight requests run to their (hyper-
    /// default) timeouts.
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
    {
        crate::tls::init_crypto_once();

        let notify = Arc::new(Notify::new());

        // Shutdown task: when `shutdown` resolves, wake every accept
        // loop via `notify_waiters`.
        let notify_for_shutdown = notify.clone();
        let shutdown_task = tokio::spawn(async move {
            shutdown.await;
            notify_for_shutdown.notify_waiters();
        });

        let redirect_port = https.as_ref().map(|h| h.public_port);
        let tls_acceptor = https
            .as_ref()
            .map(|h| TlsAcceptor::from(build_server_config(h.cert_store.clone())));
        let https_listener_opt = https.map(|h| h.listener);

        // HTTP accept loop.
        let http_router = router.clone();
        let http_resolver = backend_resolver.clone();
        let http_notify = notify.clone();
        let http_accept = tokio::spawn(async move {
            let notified = http_notify.notified();
            tokio::pin!(notified);
            loop {
                tokio::select! {
                    biased;
                    () = &mut notified => break,
                    accepted = http_listener.accept() => {
                        match accepted {
                            Ok((stream, peer)) => {
                                let router = http_router.clone();
                                let resolver = http_resolver.clone();
                                tokio::spawn(serve_http_connection(
                                    stream, peer, router, resolver, redirect_port,
                                ));
                            }
                            Err(e) => {
                                tracing::debug!(
                                    target: "yerd_proxy::accept",
                                    error = %e,
                                    "HTTP accept failed",
                                );
                            }
                        }
                    }
                }
            }
        });

        // HTTPS accept loop (if configured).
        let tls_accept =
            if let (Some(listener), Some(acceptor)) = (https_listener_opt, tls_acceptor) {
                let router = router.clone();
                let resolver = backend_resolver.clone();
                let notify_https = notify.clone();
                Some(tokio::spawn(async move {
                    let notified = notify_https.notified();
                    tokio::pin!(notified);
                    loop {
                        tokio::select! {
                            biased;
                            () = &mut notified => break,
                            accepted = listener.accept() => {
                                match accepted {
                                    Ok((stream, peer)) => {
                                        let router = router.clone();
                                        let resolver = resolver.clone();
                                        let acceptor = acceptor.clone();
                                        tokio::spawn(serve_https_connection(
                                            stream, peer, router, resolver, acceptor,
                                        ));
                                    }
                                    Err(e) => {
                                        tracing::debug!(
                                            target: "yerd_proxy::accept",
                                            error = %e,
                                            "HTTPS accept failed",
                                        );
                                    }
                                }
                            }
                        }
                    }
                }))
            } else {
                None
            };

        let _ = http_accept.await;
        if let Some(h) = tls_accept {
            let _ = h.await;
        }
        let _ = shutdown_task.await;
        Ok(())
    }
}

async fn serve_http_connection<R: BackendResolver>(
    stream: tokio::net::TcpStream,
    peer: SocketAddr,
    router: SharedRouter,
    resolver: Arc<R>,
    redirect_port: Option<u16>,
) {
    let server_addr = stream
        .local_addr()
        .unwrap_or_else(|_| "0.0.0.0:0".parse().unwrap_or(([0, 0, 0, 0], 0).into()));
    let io = TokioIo::new(stream);
    let svc = service_fn(move |req| {
        handle_request(
            req,
            peer,
            server_addr,
            Listener::Http,
            router.clone(),
            resolver.clone(),
            redirect_port,
        )
    });
    let conn = hyper::server::conn::http1::Builder::new()
        .serve_connection(io, svc)
        .with_upgrades();
    let _ = conn.await;
}

async fn serve_https_connection<R: BackendResolver>(
    stream: tokio::net::TcpStream,
    peer: SocketAddr,
    router: SharedRouter,
    resolver: Arc<R>,
    acceptor: TlsAcceptor,
) {
    let server_addr = stream
        .local_addr()
        .unwrap_or_else(|_| "0.0.0.0:0".parse().unwrap_or(([0, 0, 0, 0], 0).into()));
    let tls = match acceptor.accept(stream).await {
        Ok(t) => t,
        Err(e) => {
            tracing::debug!(
                target: "yerd_proxy::tls",
                error = %e,
                "TLS handshake failed"
            );
            return;
        }
    };
    let io = TokioIo::new(tls);
    let svc = service_fn(move |req| {
        handle_request(
            req,
            peer,
            server_addr,
            Listener::Https,
            router.clone(),
            resolver.clone(),
            None,
        )
    });
    let conn = hyper::server::conn::http1::Builder::new()
        .serve_connection(io, svc)
        .with_upgrades();
    let _ = conn.await;
}

/// Service entry point. Infallible — internal errors translate to 5xx
/// responses so hyper's connection loop keeps going.
async fn handle_request<R: BackendResolver>(
    req: Request<Incoming>,
    peer_addr: SocketAddr,
    server_addr: SocketAddr,
    listener: Listener,
    router: SharedRouter,
    resolver: Arc<R>,
    redirect_port: Option<u16>,
) -> Result<Response<BoxBody>, std::convert::Infallible> {
    match dispatch(
        req,
        peer_addr,
        server_addr,
        listener,
        router,
        resolver,
        redirect_port,
    )
    .await
    {
        Ok(resp) => Ok(resp),
        Err(e) => {
            tracing::warn!(
                target: "yerd_proxy::request",
                error = %e,
                "proxy request failed",
            );
            Ok(internal_error_response())
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn dispatch<R: BackendResolver>(
    req: Request<Incoming>,
    peer_addr: SocketAddr,
    server_addr: SocketAddr,
    listener: Listener,
    router: SharedRouter,
    resolver: Arc<R>,
    redirect_port: Option<u16>,
) -> Result<Response<BoxBody>, ProxyError> {
    let Some(host) = req
        .headers()
        .get(HOST)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
    else {
        return Ok(bad_request_response());
    };

    // Resolve the request to a site (normal Host path) or to a synthetic
    // response (the unbound `localhost` fallback). The read guard is held only
    // long enough to clone the matched Site / snapshot the picker list, then
    // dropped before any await — the daemon's mutation path is the only writer.
    //
    // `unbound` is set when the request was routed without the OS `.test`
    // resolver — via `http://localhost:8080` using the `X-Yerd-Site` header, a
    // `/~domain` switch, or the pin cookie. In that mode the origin is plain
    // http on localhost (there is no localhost cert), so the HTTP→HTTPS
    // redirect below is skipped.
    let (site, unbound) = match resolve_request(&router, &req, &host).await? {
        Routed::Site { site, unbound } => (site, unbound),
        Routed::Respond(resp) => return Ok(resp),
    };
    // Serve from the site's resolved web root (e.g. `<root>/public` for
    // Laravel), falling back to the document root when no subpath is set.
    let document_root = site.served_root();

    // HTTP → HTTPS redirect (skipped in unbound mode — see above).
    if matches!(listener, Listener::Http) && !unbound {
        if let (true, Some(port)) = (site.secure(), redirect_port) {
            let pq = req
                .uri()
                .path_and_query()
                .map_or("/", http::uri::PathAndQuery::as_str);
            let loc = build_redirect_uri(&host, pq, port);
            let resp = Response::builder()
                .status(StatusCode::MOVED_PERMANENTLY)
                .header(
                    LOCATION,
                    HeaderValue::from_str(&loc).map_err(|_| ProxyError::BackendProtocol {
                        source: std::io::Error::other("invalid redirect URI"),
                    })?,
                )
                .body(empty_body())
                .map_err(|_| ProxyError::BackendProtocol {
                    source: std::io::Error::other("redirect response build failed"),
                })?;
            return Ok(resp);
        }
    }

    // Resolve backend (calls into yerd-php at the daemon level).
    let backend = resolver.backend_for(&site).await.map_err(|e| match e {
        ProxyError::BackendResolver { .. }
        | ProxyError::BackendConnect { .. }
        | ProxyError::BackendProtocol { .. } => e,
        other => ProxyError::BackendResolver {
            host: host.clone(),
            source: Box::new(other),
        },
    })?;

    let https = matches!(listener, Listener::Https);

    // Upgrade dispatch.
    if upgrade::is_upgrade(req.headers()) {
        return match backend {
            Backend::FrankenPhp { addr } => upgrade::forward(req, addr).await,
            Backend::PhpFpm { .. } | Backend::PhpFpmTcp { .. } => Ok(fcgi::upgrade_not_supported()),
        };
    }

    match backend {
        // FrankenPHP is a full HTTP server and serves its own static files.
        Backend::FrankenPhp { addr } => http_fwd::forward(req, addr).await,
        bk @ (Backend::PhpFpm { .. } | Backend::PhpFpmTcp { .. }) => {
            // `try_files`: a request that maps to a real file under the served
            // root is served directly (so favicons/CSS/JS/images load); anything
            // else falls through to the PHP front controller (`index.php`).
            if let Some(resp) =
                static_file::try_serve(req.method(), req.uri().path(), &document_root).await
            {
                return Ok(resp);
            }
            fcgi::forward(req, bk, document_root, server_addr, peer_addr, https).await
        }
    }
}

/// Outcome of resolving a request: either a site to serve, or a ready-made
/// response (404 / 303 switch / picker) to return as-is.
enum Routed {
    /// A site to forward to. `unbound` skips the HTTP→HTTPS redirect.
    Site { site: Site, unbound: bool },
    /// A synthetic response to return directly.
    Respond(Response<BoxBody>),
}

/// Resolve a request to a [`Routed`] outcome.
///
/// Normal `Host` resolution is tried first (unchanged behaviour). On a miss, if
/// the Host is loopback, the unbound (`localhost`) fallback is applied via
/// [`classify_unbound`]; otherwise it's a 404 as before.
async fn resolve_request(
    router: &SharedRouter,
    req: &Request<Incoming>,
    host: &str,
) -> Result<Routed, ProxyError> {
    let guard = router.read().await;
    if let Some(site) = guard.resolve(host).cloned() {
        return Ok(Routed::Site {
            site,
            unbound: false,
        });
    }
    if !unbound::is_loopback_host(host) {
        return Ok(Routed::Respond(not_found_response()));
    }

    let site_header = site_header_value(req.headers());
    let cookie = req.headers().get(COOKIE).and_then(|v| v.to_str().ok());
    let accept = req.headers().get(ACCEPT).and_then(|v| v.to_str().ok());

    match classify_unbound(
        &guard,
        req.method(),
        req.uri().path(),
        req.uri().query(),
        site_header,
        cookie,
        accept,
    ) {
        UnboundDecision::Serve(site) => Ok(Routed::Site {
            site,
            unbound: true,
        }),
        UnboundDecision::Switch { name, location } => {
            Ok(Routed::Respond(switch_response(&name, &location)?))
        }
        UnboundDecision::Picker { dest, clear } => {
            // Snapshot the site list (owned) while the guard is held, then drop
            // it before rendering — no await crosses the guard.
            let tld = guard.config().tld().to_owned();
            let summaries: Vec<(String, bool, &'static str)> = guard
                .iter()
                .map(|s| (s.name().to_owned(), s.secure(), kind_label(s.kind())))
                .collect();
            drop(guard);
            let picker_sites: Vec<PickerSite<'_>> = summaries
                .iter()
                .map(|(name, secure, kind)| PickerSite {
                    name,
                    secure: *secure,
                    kind,
                })
                .collect();
            let body = unbound::render_picker(&tld, &picker_sites, &dest);
            Ok(Routed::Respond(picker_response(body, clear)?))
        }
        UnboundDecision::NotFound { clear } => Ok(Routed::Respond(unbound_not_found(clear)?)),
    }
}

/// One of the ways an unbound (resolver-off, loopback) request resolves to a
/// site or a synthetic response.
enum UnboundDecision {
    /// Serve this site directly — a pin-cookie hit or an `X-Yerd-Site` header.
    Serve(Site),
    /// `303` to `location`, setting the pin cookie to `name`.
    Switch { name: String, location: String },
    /// Render the picker for `dest`; `clear` also drops a stale pin cookie.
    Picker { dest: String, clear: bool },
    /// `404`; `clear` also drops a stale pin cookie.
    NotFound { clear: bool },
}

/// Classify a loopback request that didn't resolve via the normal `Host` path.
///
/// Priority: explicit `X-Yerd-Site` header → `/~domain` switch → pin cookie →
/// picker (browser navigations) / `404` (everything else). Pure: returns owned
/// data so the caller can drop the router guard immediately.
fn classify_unbound(
    router: &SiteRouter,
    method: &Method,
    path: &str,
    query: Option<&str>,
    site_header: Option<&str>,
    cookie: Option<&str>,
    accept: Option<&str>,
) -> UnboundDecision {
    let is_html_get = *method == Method::GET && accept.is_some_and(|a| a.contains("text/html"));

    // 0. Explicit per-request site header (API clients): no cookie, no redirect.
    if let Some(value) = site_header.map(str::trim).filter(|v| !v.is_empty()) {
        return match router.resolve(value).or_else(|| router.get(value)) {
            Some(site) => UnboundDecision::Serve(site.clone()),
            None => UnboundDecision::NotFound { clear: false },
        };
    }

    // 1. Explicit /~domain switch (pins the origin via the cookie).
    if let Some(sw) = unbound::parse_switch(path) {
        if let Some(site) = router.resolve(sw.domain) {
            return UnboundDecision::Switch {
                name: site.name().to_owned(),
                location: join_path_query(sw.remainder, query),
            };
        }
        // Unknown site → picker for browsers, else 404.
        return decide_picker(is_html_get, join_path_query(sw.remainder, query), false);
    }

    // 2. Existing pin cookie.
    if let Some(name) = cookie.and_then(unbound::parse_cookie_site) {
        if let Some(site) = router.get(name) {
            return UnboundDecision::Serve(site.clone());
        }
        // Stale cookie (site removed) — clear it on the way out.
        return decide_picker(is_html_get, join_path_query(path, query), true);
    }

    // 3. No pin yet.
    decide_picker(is_html_get, join_path_query(path, query), false)
}

/// Show the picker for browser navigations; otherwise 404 (so asset/XHR/API
/// stray traffic never receives HTML).
fn decide_picker(is_html_get: bool, dest: String, clear: bool) -> UnboundDecision {
    if is_html_get {
        UnboundDecision::Picker { dest, clear }
    } else {
        UnboundDecision::NotFound { clear }
    }
}

/// Join a path with an optional non-empty query string. The path is normalised
/// to a same-origin absolute path first ([`unbound::sanitize_dest`]) so a
/// protocol-relative remainder can't become an off-origin redirect/href.
fn join_path_query(path: &str, query: Option<&str>) -> String {
    let path = unbound::sanitize_dest(path);
    match query {
        Some(q) if !q.is_empty() => format!("{path}?{q}"),
        _ => path.into_owned(),
    }
}

/// Read the per-request site directive header (`X-Yerd-Site`, or its dash-free
/// alias), if present and valid UTF-8.
fn site_header_value(headers: &http::HeaderMap) -> Option<&str> {
    headers
        .get(unbound::SITE_HEADER)
        .or_else(|| headers.get(unbound::SITE_HEADER_ALIAS))
        .and_then(|v| v.to_str().ok())
}

/// Human label for a site kind, used in the picker rows.
fn kind_label(kind: SiteKind) -> &'static str {
    match kind {
        SiteKind::Parked => "parked",
        SiteKind::Linked => "linked",
    }
}

/// Build a `ProxyError` for a (practically unreachable) synthetic-response
/// construction failure.
fn synthetic_error(msg: &'static str) -> ProxyError {
    ProxyError::BackendProtocol {
        source: std::io::Error::other(msg),
    }
}

/// `303 See Other` to `location`, pinning the origin to `name` via `Set-Cookie`.
fn switch_response(name: &str, location: &str) -> Result<Response<BoxBody>, ProxyError> {
    let set_cookie = unbound::build_set_cookie(name);
    Response::builder()
        .status(StatusCode::SEE_OTHER)
        .header(
            LOCATION,
            HeaderValue::from_str(location)
                .map_err(|_| synthetic_error("invalid switch location"))?,
        )
        .header(
            SET_COOKIE,
            HeaderValue::from_str(&set_cookie).map_err(|_| synthetic_error("invalid pin cookie"))?,
        )
        // The same localhost URL serves different sites depending on the pin, so
        // never let an intermediary cache this redirect.
        .header(CACHE_CONTROL, "no-store")
        .header(SERVER, server_header())
        .body(empty_body())
        .map_err(|_| synthetic_error("switch response build failed"))
}

/// `200` picker page; clears a stale pin cookie when `clear` is set.
fn picker_response(body_html: String, clear: bool) -> Result<Response<BoxBody>, ProxyError> {
    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header(SERVER, server_header())
        .header(CACHE_CONTROL, "no-store")
        .header(CONTENT_TYPE, "text/html; charset=utf-8");
    if clear {
        builder = builder.header(
            SET_COOKIE,
            HeaderValue::from_str(&unbound::build_clear_cookie())
                .map_err(|_| synthetic_error("invalid clear cookie"))?,
        );
    }
    builder
        .body(owned_bytes_body(body_html.into_bytes()))
        .map_err(|_| synthetic_error("picker response build failed"))
}

/// `404` for unbound requests, clearing a stale pin cookie when `clear` is set.
fn unbound_not_found(clear: bool) -> Result<Response<BoxBody>, ProxyError> {
    if !clear {
        return Ok(not_found_response());
    }
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header(SERVER, server_header())
        .header(CACHE_CONTROL, "no-store")
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(
            SET_COOKIE,
            HeaderValue::from_str(&unbound::build_clear_cookie())
                .map_err(|_| synthetic_error("invalid clear cookie"))?,
        )
        .body(bytes_body(b"No site matches this Host.\n"))
        .map_err(|_| synthetic_error("not found response build failed"))
}

/// `Server: yerd` — stamped on every synthetic (proxy-originated) response so
/// the macOS port-redirect probe can confirm a connection to 80/443 actually
/// reaches *this* proxy, not some other listener. See [`yerd_core::PROXY_SERVER_ID`].
fn server_header() -> HeaderValue {
    HeaderValue::from_static(yerd_core::PROXY_SERVER_ID)
}

fn bad_request_response() -> Response<BoxBody> {
    Response::builder()
        .status(StatusCode::BAD_REQUEST)
        .header(SERVER, server_header())
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(bytes_body(b"Missing or invalid Host header.\n"))
        .unwrap_or_else(|_| Response::new(empty_body()))
}

fn not_found_response() -> Response<BoxBody> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header(SERVER, server_header())
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(bytes_body(b"No site matches this Host.\n"))
        .unwrap_or_else(|_| Response::new(empty_body()))
}

fn internal_error_response() -> Response<BoxBody> {
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .header(SERVER, server_header())
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(bytes_body(b"Proxy internal error.\n"))
        .unwrap_or_else(|_| Response::new(empty_body()))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod unbound_tests {
    use super::*;
    use yerd_core::{PhpVersion, RouterConfig};

    const HTML: Option<&str> = Some("text/html,application/xhtml+xml");

    fn router() -> SiteRouter {
        let cfg = RouterConfig::new("test").unwrap();
        SiteRouter::from_sites(
            cfg,
            [
                Site::parked("app", "/srv/app", PhpVersion::new(8, 3)).unwrap(),
                Site::linked("blog", "/srv/blog", PhpVersion::new(8, 3)).unwrap(),
            ],
        )
        .unwrap()
    }

    /// GET classification with the shared router.
    fn classify(
        path: &str,
        query: Option<&str>,
        header: Option<&str>,
        cookie: Option<&str>,
        accept: Option<&str>,
    ) -> UnboundDecision {
        classify_unbound(&router(), &Method::GET, path, query, header, cookie, accept)
    }

    fn served_name(d: UnboundDecision) -> String {
        match d {
            UnboundDecision::Serve(s) => s.name().to_owned(),
            other => panic!("expected Serve, got {}", variant(&other)),
        }
    }

    fn variant(d: &UnboundDecision) -> &'static str {
        match d {
            UnboundDecision::Serve(_) => "Serve",
            UnboundDecision::Switch { .. } => "Switch",
            UnboundDecision::Picker { .. } => "Picker",
            UnboundDecision::NotFound { .. } => "NotFound",
        }
    }

    #[test]
    fn header_routes_directly() {
        // Domain form and bare-label form both resolve; no cookie, no redirect.
        assert_eq!(served_name(classify("/", None, Some("app.test"), None, None)), "app");
        assert_eq!(served_name(classify("/", None, Some("blog"), None, None)), "blog");
    }

    #[test]
    fn header_unknown_is_404_not_picker() {
        match classify("/", None, Some("ghost.test"), None, HTML) {
            UnboundDecision::NotFound { clear } => assert!(!clear),
            other => panic!("expected NotFound, got {}", variant(&other)),
        }
    }

    #[test]
    fn switch_sets_cookie_and_location() {
        match classify("/~app.test", None, None, None, HTML) {
            UnboundDecision::Switch { name, location } => {
                assert_eq!(name, "app");
                assert_eq!(location, "/");
            }
            other => panic!("expected Switch, got {}", variant(&other)),
        }
        match classify("/~app.test/x", Some("y=1"), None, None, None) {
            UnboundDecision::Switch { name, location } => {
                assert_eq!(name, "app");
                assert_eq!(location, "/x?y=1");
            }
            other => panic!("expected Switch, got {}", variant(&other)),
        }
    }

    #[test]
    fn switch_works_for_any_method() {
        let d = classify_unbound(
            &router(),
            &Method::POST,
            "/~app.test/login",
            None,
            None,
            None,
            None,
        );
        assert!(matches!(d, UnboundDecision::Switch { .. }));
    }

    #[test]
    fn switch_beats_existing_cookie() {
        assert_eq!(
            match classify("/~blog.test/p", None, None, Some("yerd-site=app"), HTML) {
                UnboundDecision::Switch { name, .. } => name,
                other => panic!("expected Switch, got {}", variant(&other)),
            },
            "blog"
        );
    }

    #[test]
    fn unknown_switch_degrades_to_picker_for_browsers() {
        match classify("/~nope.test/p", None, None, None, HTML) {
            UnboundDecision::Picker { dest, clear } => {
                assert_eq!(dest, "/p");
                assert!(!clear);
            }
            other => panic!("expected Picker, got {}", variant(&other)),
        }
        // Non-browser → 404.
        assert!(matches!(
            classify("/~nope.test/p", None, None, None, Some("application/json")),
            UnboundDecision::NotFound { .. }
        ));
    }

    #[test]
    fn cookie_serves_pinned_site() {
        assert_eq!(
            served_name(classify("/dashboard", None, None, Some("yerd-site=app"), None)),
            "app"
        );
    }

    #[test]
    fn stale_cookie_clears_and_picks_or_404s() {
        match classify("/x", None, None, Some("yerd-site=ghost"), HTML) {
            UnboundDecision::Picker { dest, clear } => {
                assert_eq!(dest, "/x");
                assert!(clear);
            }
            other => panic!("expected Picker, got {}", variant(&other)),
        }
        match classify("/x.css", None, None, Some("yerd-site=ghost"), Some("text/css")) {
            UnboundDecision::NotFound { clear } => assert!(clear),
            other => panic!("expected NotFound, got {}", variant(&other)),
        }
    }

    #[test]
    fn no_cookie_navigation_shows_picker_with_dest() {
        match classify("/example", Some("x=1"), None, None, HTML) {
            UnboundDecision::Picker { dest, clear } => {
                assert_eq!(dest, "/example?x=1");
                assert!(!clear);
            }
            other => panic!("expected Picker, got {}", variant(&other)),
        }
    }

    #[test]
    fn no_cookie_asset_request_is_404() {
        assert!(matches!(
            classify("/app.css", None, None, None, Some("text/css,*/*;q=0.1")),
            UnboundDecision::NotFound { clear: false }
        ));
    }

    #[test]
    fn header_beats_switch() {
        // X-Yerd-Site wins over a /~ switch path.
        assert_eq!(
            served_name(classify("/~blog.test/p", None, Some("app.test"), None, HTML)),
            "app"
        );
    }

    #[test]
    fn blank_header_falls_through() {
        // Whitespace-only header is ignored → normal classification (picker).
        match classify("/", None, Some("   "), None, HTML) {
            UnboundDecision::Picker { dest, .. } => assert_eq!(dest, "/"),
            other => panic!("expected Picker, got {}", variant(&other)),
        }
    }

    #[test]
    fn switch_location_is_same_origin() {
        // Open-redirect guard: a protocol-relative remainder is normalised.
        match classify("/~app.test//evil.com", None, None, None, HTML) {
            UnboundDecision::Switch { location, .. } => assert_eq!(location, "/evil.com"),
            other => panic!("expected Switch, got {}", variant(&other)),
        }
        // And the picker dest for a protocol-relative path is normalised too.
        match classify("//evil.com", None, None, None, HTML) {
            UnboundDecision::Picker { dest, .. } => assert_eq!(dest, "/evil.com"),
            other => panic!("expected Picker, got {}", variant(&other)),
        }
    }

    #[test]
    fn site_header_value_prefers_canonical_then_alias() {
        let mut h = http::HeaderMap::new();
        assert_eq!(site_header_value(&h), None);
        h.insert("x-yerdsite", "blog".parse().unwrap());
        assert_eq!(site_header_value(&h), Some("blog"));
        h.insert("x-yerd-site", "app.test".parse().unwrap());
        assert_eq!(site_header_value(&h), Some("app.test"));
    }

    fn header(resp: &Response<BoxBody>, name: http::header::HeaderName) -> Option<String> {
        resp.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    }

    #[test]
    fn switch_response_shape() {
        let resp = switch_response("app", "/x?y=1").unwrap();
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(header(&resp, LOCATION).as_deref(), Some("/x?y=1"));
        assert_eq!(header(&resp, CACHE_CONTROL).as_deref(), Some("no-store"));
        assert!(header(&resp, SET_COOKIE)
            .unwrap()
            .contains("yerd-site=app"));
    }

    #[test]
    fn picker_response_shape() {
        let plain = picker_response("<html></html>".to_owned(), false).unwrap();
        assert_eq!(plain.status(), StatusCode::OK);
        assert_eq!(header(&plain, CACHE_CONTROL).as_deref(), Some("no-store"));
        assert!(header(&plain, CONTENT_TYPE).unwrap().contains("text/html"));
        assert!(header(&plain, SET_COOKIE).is_none());

        let cleared = picker_response("<html></html>".to_owned(), true).unwrap();
        assert!(header(&cleared, SET_COOKIE).unwrap().contains("Max-Age=0"));
    }

    #[test]
    fn unbound_not_found_clear_variant_carries_no_store_and_clear_cookie() {
        let cleared = unbound_not_found(true).unwrap();
        assert_eq!(cleared.status(), StatusCode::NOT_FOUND);
        assert_eq!(header(&cleared, CACHE_CONTROL).as_deref(), Some("no-store"));
        assert!(header(&cleared, SET_COOKIE).unwrap().contains("Max-Age=0"));

        // The non-clearing variant is the shared 404 (no cookie touched).
        let plain = unbound_not_found(false).unwrap();
        assert_eq!(plain.status(), StatusCode::NOT_FOUND);
        assert!(header(&plain, SET_COOKIE).is_none());
    }
}
