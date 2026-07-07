//! `ProxyServer::serve` - accept loops + per-connection hyper service.

use std::future::Future;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU16, Ordering};
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
    bytes_body, empty_body, fcgi, http as http_fwd, owned_bytes_body, script_file, static_file,
    upgrade, BoxBody,
};
use crate::pure::cgi_params;
use crate::pure::query;
use crate::pure::redirect::build_redirect_uri;
use crate::pure::unbound::{self, PickerSite};
use crate::tls::build_server_config;
use crate::traits::{BackendResolver, CertStore, LoginTokenConsumer};

/// Query param carrying the one-click `WordPress` login token (see
/// `dispatch`'s interception branch below).
const LOGIN_TOKEN_PARAM: &str = "yerd_login_token";

/// Router shared between the proxy's request path (read) and the daemon's
/// mutation path (write-replace). Reads are brief and uncontended - each
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
    /// Public port the HTTP→HTTPS redirect should target - not
    /// necessarily what `listener.local_addr()` reports (rootless mode may
    /// bind 8443 while a live privileged-port redirect, e.g. `yerd elevate
    /// ports`'s macOS `pf` rule, makes 443 the port to advertise instead).
    /// Shared (not a plain `u16`) so the daemon can flip it as that redirect
    /// goes up or down, without restarting the proxy to pick up the change.
    pub public_port: Arc<AtomicU16>,
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
    #[allow(clippy::too_many_arguments)]
    pub async fn serve<R, C, S, L>(
        http_listener: TcpListener,
        https: Option<HttpsBinding<C>>,
        router: SharedRouter,
        backend_resolver: Arc<R>,
        login_tokens: Arc<L>,
        login_prepend_script: Option<std::path::PathBuf>,
        shutdown: S,
    ) -> Result<(), ProxyError>
    where
        R: BackendResolver,
        C: CertStore,
        S: Future<Output = ()> + Send + 'static,
        L: LoginTokenConsumer,
    {
        crate::tls::init_crypto_once();

        let notify = Arc::new(Notify::new());

        let notify_for_shutdown = notify.clone();
        let shutdown_task = tokio::spawn(async move {
            shutdown.await;
            notify_for_shutdown.notify_waiters();
        });

        let redirect_port = https.as_ref().map(|h| h.public_port.clone());
        let tls_acceptor = https
            .as_ref()
            .map(|h| TlsAcceptor::from(build_server_config(h.cert_store.clone())));
        let https_listener_opt = https.map(|h| h.listener);

        let http_router = router.clone();
        let http_resolver = backend_resolver.clone();
        let http_login_tokens = login_tokens.clone();
        let http_login_prepend = login_prepend_script.clone();
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
                                let login_tokens = http_login_tokens.clone();
                                let login_prepend = http_login_prepend.clone();
                                tokio::spawn(serve_http_connection(
                                    stream, peer, router, resolver, login_tokens, login_prepend, redirect_port.clone(),
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

        let tls_accept = if let (Some(listener), Some(acceptor)) =
            (https_listener_opt, tls_acceptor)
        {
            let router = router.clone();
            let resolver = backend_resolver.clone();
            let login_tokens = login_tokens.clone();
            let login_prepend_script = login_prepend_script.clone();
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
                                    let login_tokens = login_tokens.clone();
                                    let login_prepend = login_prepend_script.clone();
                                    let acceptor = acceptor.clone();
                                    tokio::spawn(serve_https_connection(
                                        stream, peer, router, resolver, login_tokens, login_prepend, acceptor,
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

#[allow(clippy::too_many_arguments)]
async fn serve_http_connection<R: BackendResolver, L: LoginTokenConsumer>(
    stream: tokio::net::TcpStream,
    peer: SocketAddr,
    router: SharedRouter,
    resolver: Arc<R>,
    login_tokens: Arc<L>,
    login_prepend_script: Option<std::path::PathBuf>,
    redirect_port: Option<Arc<AtomicU16>>,
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
            login_tokens.clone(),
            login_prepend_script.clone(),
            redirect_port.clone(),
        )
    });
    let conn = hyper::server::conn::http1::Builder::new()
        .serve_connection(io, svc)
        .with_upgrades();
    let _ = conn.await;
}

#[allow(clippy::too_many_arguments)]
async fn serve_https_connection<R: BackendResolver, L: LoginTokenConsumer>(
    stream: tokio::net::TcpStream,
    peer: SocketAddr,
    router: SharedRouter,
    resolver: Arc<R>,
    login_tokens: Arc<L>,
    login_prepend_script: Option<std::path::PathBuf>,
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
            login_tokens.clone(),
            login_prepend_script.clone(),
            None,
        )
    });
    let conn = hyper::server::conn::http1::Builder::new()
        .serve_connection(io, svc)
        .with_upgrades();
    let _ = conn.await;
}

/// Service entry point. Infallible - internal errors translate to 5xx
/// responses so hyper's connection loop keeps going.
#[allow(clippy::too_many_arguments)]
async fn handle_request<R: BackendResolver, L: LoginTokenConsumer>(
    req: Request<Incoming>,
    peer_addr: SocketAddr,
    server_addr: SocketAddr,
    listener: Listener,
    router: SharedRouter,
    resolver: Arc<R>,
    login_tokens: Arc<L>,
    login_prepend_script: Option<std::path::PathBuf>,
    redirect_port: Option<Arc<AtomicU16>>,
) -> Result<Response<BoxBody>, std::convert::Infallible> {
    match dispatch(
        req,
        peer_addr,
        server_addr,
        listener,
        router,
        resolver,
        login_tokens,
        login_prepend_script,
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
async fn dispatch<R: BackendResolver, L: LoginTokenConsumer>(
    req: Request<Incoming>,
    peer_addr: SocketAddr,
    server_addr: SocketAddr,
    listener: Listener,
    router: SharedRouter,
    resolver: Arc<R>,
    login_tokens: Arc<L>,
    login_prepend_script: Option<std::path::PathBuf>,
    redirect_port: Option<Arc<AtomicU16>>,
) -> Result<Response<BoxBody>, ProxyError> {
    let redirect_port = redirect_port.map(|p| p.load(Ordering::Relaxed));
    let Some(host) = req
        .headers()
        .get(HOST)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
    else {
        return Ok(bad_request_response());
    };

    let (site, unbound) = match resolve_request(&router, &req, &host).await? {
        Routed::Site { site, unbound } => (site, unbound),
        Routed::Respond(resp) => return Ok(resp),
    };
    let served_root = site.served_root();
    let allowed_root = site.document_root().to_path_buf();

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

    if upgrade::is_upgrade(req.headers()) {
        return match backend {
            Backend::FrankenPhp { addr } => upgrade::forward(req, addr).await,
            Backend::PhpFpm { .. } | Backend::PhpFpmTcp { .. } => Ok(fcgi::upgrade_not_supported()),
        };
    }

    match backend {
        Backend::FrankenPhp { addr } => http_fwd::forward(req, addr).await,
        bk @ (Backend::PhpFpm { .. } | Backend::PhpFpmTcp { .. }) => {
            serve_php_fpm(
                req,
                bk,
                resolver.as_ref(),
                &site,
                login_tokens.as_ref(),
                login_prepend_script.as_deref(),
                &served_root,
                &allowed_root,
                server_addr,
                peer_addr,
                https,
            )
            .await
        }
    }
}

/// The `Backend::PhpFpm`/`PhpFpmTcp` half of [`dispatch`]: static files,
/// then a real script (gated by
/// [`BackendResolver::allows_direct_script_execution`]), then the site
/// root's `index.php`.
///
/// The one-click `WordPress` login token is consumed here via
/// [`consume_login_token_if_present`] - only ever on `/wp-admin`, only when a
/// token is both present and valid for *this* site. This runs strictly after
/// [`dispatch`]'s HTTP->HTTPS redirect check, so a secure site's token is
/// never burned by the 301 itself. On success the token is stripped from the
/// forwarded URI (never reaching PHP or logging) and `auto_prepend_file` plus
/// the resolved target user are added for this one request only.
#[allow(clippy::too_many_arguments)]
async fn serve_php_fpm<R: BackendResolver, L: LoginTokenConsumer>(
    mut req: Request<Incoming>,
    backend: Backend,
    resolver: &R,
    site: &yerd_core::Site,
    login_tokens: &L,
    login_prepend_script: Option<&std::path::Path>,
    served_root: &std::path::Path,
    allowed_root: &std::path::Path,
    server_addr: SocketAddr,
    peer_addr: SocketAddr,
    https: bool,
) -> Result<Response<BoxBody>, ProxyError> {
    let login_target_user = consume_login_token_if_present(&mut req, site.name(), login_tokens);
    let auto_login = match (&login_target_user, login_prepend_script) {
        (Some(target_user), Some(prepend_script)) => Some(cgi_params::AutoLoginParams {
            prepend_script,
            target_user: target_user.as_str(),
        }),
        _ => None,
    };

    let outcome =
        static_file::try_serve(req.method(), req.uri().path(), served_root, allowed_root).await;
    if let Some(resp) = resolve_static_outcome(outcome) {
        return Ok(resp);
    }
    let outcome =
        static_file::try_serve_index(req.method(), req.uri().path(), served_root, allowed_root)
            .await;
    if let Some(resp) = resolve_static_outcome(outcome) {
        return Ok(resp);
    }
    let script_rel =
        resolve_script_if_allowed(resolver, site, req.uri().path(), served_root, allowed_root)
            .await;
    fcgi::forward(
        req,
        backend,
        served_root.to_path_buf(),
        script_rel,
        server_addr,
        peer_addr,
        https,
        auto_login,
    )
    .await
}

fn path_and_query_or_root(uri: &http::Uri) -> &str {
    uri.path_and_query()
        .map_or("/", http::uri::PathAndQuery::as_str)
}

/// [`script_file::resolve_script`], gated by
/// [`BackendResolver::allows_direct_script_execution`] - `None` (fall back to
/// the site root's `index.php`) for any site the resolver hasn't opted in,
/// without touching the filesystem to find out.
async fn resolve_script_if_allowed<R: BackendResolver>(
    resolver: &R,
    site: &yerd_core::Site,
    uri_path: &str,
    served_root: &std::path::Path,
    allowed_root: &std::path::Path,
) -> Option<std::path::PathBuf> {
    if !resolver.allows_direct_script_execution(site).await {
        return None;
    }
    script_file::resolve_script(uri_path, served_root, allowed_root).await
}

/// One-click `WordPress` login: only ever considered on `/wp-admin`, only
/// ever acted on when a token is both present and valid for `site_name`.
/// Consuming happens here - the caller must only call this strictly after the
/// HTTP->HTTPS redirect check, so a secure site's token is never burned by
/// the 301 itself. On success, strips the token from `req`'s URI (so it never
/// reaches PHP or request logging) and returns `Some(target_user)` (`""` = no
/// preference); the caller decides what "success" means for its own request
/// (adding `auto_prepend_file`/`YERD_LOGIN_USER`).
fn consume_login_token_if_present<B, L: LoginTokenConsumer>(
    req: &mut Request<B>,
    site_name: &str,
    login_tokens: &L,
) -> Option<String> {
    if !req.uri().path().starts_with("/wp-admin") {
        return None;
    }
    let token = query::get_param(req.uri().query(), LOGIN_TOKEN_PARAM).map(str::to_owned)?;
    let target_user = login_tokens.consume(site_name, &token)?;
    let stripped = query::strip_param(path_and_query_or_root(req.uri()), LOGIN_TOKEN_PARAM);
    if let Ok(new_uri) = stripped.parse::<http::Uri>() {
        *req.uri_mut() = new_uri;
    }
    Some(target_user)
}

/// Turn a [`static_file::StaticOutcome`] into a response to return
/// immediately, or `None` to fall through to the front controller.
fn resolve_static_outcome(outcome: static_file::StaticOutcome) -> Option<Response<BoxBody>> {
    match outcome {
        static_file::StaticOutcome::Served(resp) => Some(resp),
        static_file::StaticOutcome::SymlinkEscape {
            requested_path,
            resolved,
            allowed_root,
        } => Some(static_file::symlink_escape_response(
            &requested_path,
            &resolved,
            &allowed_root,
        )),
        static_file::StaticOutcome::NotFound => None,
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
        UnboundDecision::ClearPin => Ok(Routed::Respond(clear_response()?)),
    }
}

/// One of the ways an unbound (resolver-off, loopback) request resolves to a
/// site or a synthetic response.
enum UnboundDecision {
    /// Serve this site directly - a pin-cookie hit or an `X-Yerd-Site` header.
    Serve(Site),
    /// `303` to `location`, setting the pin cookie to `name`.
    Switch { name: String, location: String },
    /// Render the picker for `dest`; `clear` also drops a stale pin cookie.
    Picker { dest: String, clear: bool },
    /// `404`; `clear` also drops a stale pin cookie.
    NotFound { clear: bool },
    /// `303` to `/`, clearing the pin cookie - the bare `/~` escape hatch.
    ClearPin,
}

/// Classify a loopback request that didn't resolve via the normal `Host` path.
///
/// Priority: explicit `X-Yerd-Site` header → bare `/~` clear → `/~domain`
/// switch → pin cookie → picker (browser navigations) / `404` (everything
/// else). Pure: returns owned data so the caller can drop the router guard
/// immediately.
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

    if let Some(value) = site_header.map(str::trim).filter(|v| !v.is_empty()) {
        return match router.resolve(value).or_else(|| router.get(value)) {
            Some(site) => UnboundDecision::Serve(site.clone()),
            None => UnboundDecision::NotFound { clear: false },
        };
    }

    if unbound::is_clear_switch(path) {
        return UnboundDecision::ClearPin;
    }

    if let Some(sw) = unbound::parse_switch(path) {
        if let Some(site) = router.resolve(sw.domain).or_else(|| router.get(sw.domain)) {
            return UnboundDecision::Switch {
                name: site.name().to_owned(),
                location: join_path_query(sw.remainder, query),
            };
        }
        return decide_picker(is_html_get, join_path_query(sw.remainder, query), false);
    }

    if let Some(name) = cookie.and_then(unbound::parse_cookie_site) {
        if let Some(site) = router.get(name) {
            return UnboundDecision::Serve(site.clone());
        }
        return decide_picker(is_html_get, join_path_query(path, query), true);
    }

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
            HeaderValue::from_str(&set_cookie)
                .map_err(|_| synthetic_error("invalid pin cookie"))?,
        )
        .header(CACHE_CONTROL, "no-store")
        .header(SERVER, server_header())
        .body(empty_body())
        .map_err(|_| synthetic_error("switch response build failed"))
}

/// `303 See Other` to `/`, clearing the pin cookie - the bare `/~` escape
/// hatch back to the picker.
fn clear_response() -> Result<Response<BoxBody>, ProxyError> {
    Response::builder()
        .status(StatusCode::SEE_OTHER)
        .header(LOCATION, HeaderValue::from_static("/"))
        .header(
            SET_COOKIE,
            HeaderValue::from_str(&unbound::build_clear_cookie())
                .map_err(|_| synthetic_error("invalid clear cookie"))?,
        )
        .header(CACHE_CONTROL, "no-store")
        .header(SERVER, server_header())
        .body(empty_body())
        .map_err(|_| synthetic_error("clear response build failed"))
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

/// `404` for unbound requests. Always `Cache-Control: no-store` - in unbound
/// mode the same localhost URL serves different sites by pin, so a negative
/// response must never be cached and resurface after the user pins a site.
/// Clears a stale pin cookie when `clear` is set.
fn unbound_not_found(clear: bool) -> Result<Response<BoxBody>, ProxyError> {
    let mut builder = Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header(SERVER, server_header())
        .header(CACHE_CONTROL, "no-store")
        .header(CONTENT_TYPE, "text/plain; charset=utf-8");
    if clear {
        builder = builder.header(
            SET_COOKIE,
            HeaderValue::from_str(&unbound::build_clear_cookie())
                .map_err(|_| synthetic_error("invalid clear cookie"))?,
        );
    }
    builder
        .body(bytes_body(b"No site matches this Host.\n"))
        .map_err(|_| synthetic_error("not found response build failed"))
}

/// `Server: yerd` - stamped on every synthetic (proxy-originated) response so
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
            UnboundDecision::ClearPin => "ClearPin",
        }
    }

    #[test]
    fn header_routes_directly() {
        assert_eq!(
            served_name(classify("/", None, Some("app.test"), None, None)),
            "app"
        );
        assert_eq!(
            served_name(classify("/", None, Some("blog"), None, None)),
            "blog"
        );
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
    fn bare_tilde_clears_pin() {
        for path in ["/~", "/~/"] {
            assert!(
                matches!(
                    classify(path, None, None, None, HTML),
                    UnboundDecision::ClearPin
                ),
                "path {path:?} with no cookie"
            );
            assert!(
                matches!(
                    classify(path, None, None, Some("yerd-site=app"), HTML),
                    UnboundDecision::ClearPin
                ),
                "path {path:?} should clear an existing pin"
            );
        }
    }

    #[test]
    fn header_beats_bare_clear() {
        assert_eq!(
            served_name(classify("/~", None, Some("app.test"), None, HTML)),
            "app"
        );
    }

    #[test]
    fn domain_switch_not_confused_with_clear() {
        for path in ["/~app.test", "/~app.test/"] {
            assert!(
                matches!(
                    classify(path, None, None, None, HTML),
                    UnboundDecision::Switch { .. }
                ),
                "path {path:?} should still switch"
            );
        }
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
        assert!(matches!(
            classify("/~nope.test/p", None, None, None, Some("application/json")),
            UnboundDecision::NotFound { .. }
        ));
    }

    #[test]
    fn cookie_serves_pinned_site() {
        assert_eq!(
            served_name(classify(
                "/dashboard",
                None,
                None,
                Some("yerd-site=app"),
                None
            )),
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
        match classify(
            "/x.css",
            None,
            None,
            Some("yerd-site=ghost"),
            Some("text/css"),
        ) {
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
        assert_eq!(
            served_name(classify(
                "/~blog.test/p",
                None,
                Some("app.test"),
                None,
                HTML
            )),
            "app"
        );
    }

    #[test]
    fn blank_header_falls_through() {
        match classify("/", None, Some("   "), None, HTML) {
            UnboundDecision::Picker { dest, .. } => assert_eq!(dest, "/"),
            other => panic!("expected Picker, got {}", variant(&other)),
        }
    }

    #[test]
    fn switch_location_is_same_origin() {
        match classify("/~app.test//evil.com", None, None, None, HTML) {
            UnboundDecision::Switch { location, .. } => assert_eq!(location, "/evil.com"),
            other => panic!("expected Switch, got {}", variant(&other)),
        }
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
        assert!(header(&resp, SET_COOKIE).unwrap().contains("yerd-site=app"));
    }

    #[test]
    fn clear_response_shape() {
        let resp = clear_response().unwrap();
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(header(&resp, LOCATION).as_deref(), Some("/"));
        assert_eq!(header(&resp, CACHE_CONTROL).as_deref(), Some("no-store"));
        let set_cookie = header(&resp, SET_COOKIE).unwrap();
        assert!(set_cookie.contains("Max-Age=0"));
        assert!(set_cookie.contains("yerd-site=;"));
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

        let plain = unbound_not_found(false).unwrap();
        assert_eq!(plain.status(), StatusCode::NOT_FOUND);
        assert_eq!(header(&plain, CACHE_CONTROL).as_deref(), Some("no-store"));
        assert!(header(&plain, SET_COOKIE).is_none());
    }

    #[test]
    fn bare_label_switch_resolves_like_header() {
        match classify("/~app/dash", None, None, None, HTML) {
            UnboundDecision::Switch { name, location } => {
                assert_eq!(name, "app");
                assert_eq!(location, "/dash");
            }
            other => panic!("expected Switch, got {}", variant(&other)),
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod synthetic_response_tests {
    use super::*;

    fn header_val(resp: &Response<BoxBody>, name: http::header::HeaderName) -> Option<String> {
        resp.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    }

    #[test]
    fn join_path_query_appends_non_empty_query() {
        assert_eq!(join_path_query("/x", Some("a=1")), "/x?a=1");
    }

    #[test]
    fn join_path_query_drops_empty_or_missing_query() {
        assert_eq!(join_path_query("/x", Some("")), "/x");
        assert_eq!(join_path_query("/x", None), "/x");
    }

    /// sanitize_dest collapses a protocol-relative remainder to same-origin.
    #[test]
    fn join_path_query_normalises_protocol_relative_path() {
        assert_eq!(join_path_query("//evil.com", None), "/evil.com");
        assert_eq!(join_path_query("//evil.com", Some("q=1")), "/evil.com?q=1");
    }

    #[test]
    fn kind_label_maps_each_variant() {
        assert_eq!(kind_label(SiteKind::Parked), "parked");
        assert_eq!(kind_label(SiteKind::Linked), "linked");
    }

    #[test]
    fn synthetic_error_is_backend_protocol() {
        let e = synthetic_error("boom");
        assert!(matches!(e, ProxyError::BackendProtocol { .. }));
        assert!(e.to_string().contains("backend protocol"));
    }

    #[test]
    fn server_header_is_proxy_id() {
        assert_eq!(
            server_header().to_str().unwrap(),
            yerd_core::PROXY_SERVER_ID
        );
    }

    #[test]
    fn decide_picker_picks_for_html_get() {
        match decide_picker(true, "/x".to_owned(), false) {
            UnboundDecision::Picker { dest, clear } => {
                assert_eq!(dest, "/x");
                assert!(!clear);
            }
            _ => panic!("expected Picker"),
        }
    }

    #[test]
    fn decide_picker_404s_for_non_html() {
        match decide_picker(false, "/x".to_owned(), true) {
            UnboundDecision::NotFound { clear } => assert!(clear),
            _ => panic!("expected NotFound"),
        }
    }

    #[test]
    fn bad_request_response_shape() {
        let resp = bad_request_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            header_val(&resp, SERVER).as_deref(),
            Some(yerd_core::PROXY_SERVER_ID)
        );
        assert!(header_val(&resp, CONTENT_TYPE)
            .unwrap()
            .contains("text/plain"));
    }

    #[test]
    fn not_found_response_shape() {
        let resp = not_found_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            header_val(&resp, SERVER).as_deref(),
            Some(yerd_core::PROXY_SERVER_ID)
        );
        assert!(header_val(&resp, CONTENT_TYPE)
            .unwrap()
            .contains("text/plain"));
    }

    #[test]
    fn internal_error_response_shape() {
        let resp = internal_error_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            header_val(&resp, SERVER).as_deref(),
            Some(yerd_core::PROXY_SERVER_ID)
        );
        assert!(header_val(&resp, CONTENT_TYPE)
            .unwrap()
            .contains("text/plain"));
    }

    /// A control char in the Location makes `HeaderValue::from_str` fail,
    /// exercising the synthetic_error map_err branch (otherwise unreachable
    /// from the sanitised happy path).
    #[test]
    fn switch_response_rejects_invalid_location() {
        let err = switch_response("app", "/bad\nlocation").unwrap_err();
        assert!(matches!(err, ProxyError::BackendProtocol { .. }));
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod login_token_tests {
    use super::*;

    struct FakeConsumer {
        valid: bool,
        target_user: &'static str,
    }
    impl LoginTokenConsumer for FakeConsumer {
        fn consume(&self, _site: &str, _token: &str) -> Option<String> {
            self.valid.then(|| self.target_user.to_owned())
        }
    }

    /// A consumer with real single-use semantics: valid exactly once.
    struct OnceConsumer(std::sync::atomic::AtomicBool);
    impl LoginTokenConsumer for OnceConsumer {
        fn consume(&self, _site: &str, _token: &str) -> Option<String> {
            self.0
                .swap(false, std::sync::atomic::Ordering::SeqCst)
                .then(String::new)
        }
    }

    fn get_req(uri: &str) -> Request<()> {
        Request::builder().uri(uri).body(()).unwrap()
    }

    fn path_and_query(req: &Request<()>) -> &str {
        req.uri()
            .path_and_query()
            .map_or("/", http::uri::PathAndQuery::as_str)
    }

    #[test]
    fn valid_token_strips_query_and_returns_target_user() {
        let mut req = get_req("/wp-admin/?a=1&yerd_login_token=abc&b=2");
        let consumer = FakeConsumer {
            valid: true,
            target_user: "",
        };
        assert!(consume_login_token_if_present(&mut req, "blog", &consumer).is_some());
        assert_eq!(path_and_query(&req), "/wp-admin/?a=1&b=2");
    }

    #[test]
    fn valid_token_with_configured_user_returns_that_username() {
        let mut req = get_req("/wp-admin/?yerd_login_token=abc");
        let consumer = FakeConsumer {
            valid: true,
            target_user: "editor",
        };
        assert_eq!(
            consume_login_token_if_present(&mut req, "blog", &consumer).as_deref(),
            Some("editor")
        );
    }

    #[test]
    fn path_outside_wp_admin_is_never_considered() {
        let mut req = get_req("/?yerd_login_token=abc");
        let consumer = FakeConsumer {
            valid: true,
            target_user: "",
        };
        assert!(consume_login_token_if_present(&mut req, "blog", &consumer).is_none());
        assert_eq!(path_and_query(&req), "/?yerd_login_token=abc");
    }

    #[test]
    fn missing_token_is_declined() {
        let mut req = get_req("/wp-admin/");
        let consumer = FakeConsumer {
            valid: true,
            target_user: "",
        };
        assert!(consume_login_token_if_present(&mut req, "blog", &consumer).is_none());
    }

    #[test]
    fn invalid_token_leaves_query_untouched() {
        let mut req = get_req("/wp-admin/?yerd_login_token=abc");
        let consumer = FakeConsumer {
            valid: false,
            target_user: "",
        };
        assert!(consume_login_token_if_present(&mut req, "blog", &consumer).is_none());
        assert_eq!(path_and_query(&req), "/wp-admin/?yerd_login_token=abc");
    }

    /// The same token presented twice must only work once.
    #[test]
    fn replayed_token_is_rejected_on_second_presentation() {
        let consumer = OnceConsumer(std::sync::atomic::AtomicBool::new(true));
        let mut req1 = get_req("/wp-admin/?yerd_login_token=abc");
        assert!(consume_login_token_if_present(&mut req1, "blog", &consumer).is_some());
        let mut req2 = get_req("/wp-admin/?yerd_login_token=abc");
        assert!(consume_login_token_if_present(&mut req2, "blog", &consumer).is_none());
        assert_eq!(
            path_and_query(&req2),
            "/wp-admin/?yerd_login_token=abc",
            "a rejected replay must leave the query untouched, not strip it"
        );
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod classify_fallthrough_tests {
    use super::*;
    use yerd_core::{PhpVersion, RouterConfig, Site};

    const HTML: Option<&str> = Some("text/html");

    fn router() -> SiteRouter {
        let cfg = RouterConfig::new("test").unwrap();
        SiteRouter::from_sites(
            cfg,
            [Site::parked("app", "/srv/app", PhpVersion::new(8, 3)).unwrap()],
        )
        .unwrap()
    }

    /// A cookie that carries no `yerd-site` pin must NOT clear anything and
    /// must fall through to the no-pin picker/404 path.
    #[test]
    fn unparseable_cookie_falls_through_to_normal_classification() {
        let d = classify_unbound(
            &router(),
            &Method::GET,
            "/dash",
            None,
            None,
            Some("session=abc; theme=dark"),
            HTML,
        );
        match d {
            UnboundDecision::Picker { dest, clear } => {
                assert_eq!(dest, "/dash");
                assert!(!clear);
            }
            other => panic!("expected Picker, got {other:?}"),
        }
    }

    /// POST with an HTML Accept is still not a browser navigation we render a
    /// picker for, so it must 404 (no HTML to non-GET traffic).
    #[test]
    fn non_get_navigation_without_pin_is_not_found() {
        let d = classify_unbound(&router(), &Method::POST, "/", None, None, None, HTML);
        assert!(matches!(d, UnboundDecision::NotFound { clear: false }));
    }

    /// The dotted `app.test` form resolves through `router.resolve` (not the
    /// bare-label `get` fallback) and serves directly.
    #[test]
    fn header_domain_form_resolves_via_resolver() {
        let d = classify_unbound(
            &router(),
            &Method::GET,
            "/",
            None,
            Some("app.test"),
            None,
            None,
        );
        match d {
            UnboundDecision::Serve(s) => assert_eq!(s.name(), "app"),
            other => panic!("expected Serve, got {other:?}"),
        }
    }

    impl std::fmt::Debug for UnboundDecision {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            let label = match self {
                UnboundDecision::Serve(_) => "Serve",
                UnboundDecision::Switch { .. } => "Switch",
                UnboundDecision::Picker { .. } => "Picker",
                UnboundDecision::NotFound { .. } => "NotFound",
                UnboundDecision::ClearPin => "ClearPin",
            };
            f.write_str(label)
        }
    }
}
