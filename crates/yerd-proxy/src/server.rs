//! `ProxyServer::serve` — accept loops + per-connection hyper service.

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;

use http::header::{CONTENT_TYPE, HOST, LOCATION};
use http::{HeaderValue, StatusCode};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tokio_rustls::TlsAcceptor;

use crate::backend::Backend;
use crate::error::ProxyError;
use crate::forward::{bytes_body, empty_body, fcgi, http as http_fwd, upgrade, BoxBody};
use crate::pure::redirect::build_redirect_uri;
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

    // Take a read guard only long enough to resolve + clone the Site, then
    // drop it before any await. (Site: Clone is cheap — small strings and
    // PathBufs.) The daemon's mutation path is the only writer.
    let guard = router.read().await;
    let site = match guard.resolve(&host) {
        Some(s) => s.clone(),
        None => return Ok(not_found_response()),
    };
    drop(guard);
    let document_root = site.document_root().to_path_buf();

    // HTTP → HTTPS redirect.
    if matches!(listener, Listener::Http) {
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
        Backend::FrankenPhp { addr } => http_fwd::forward(req, addr).await,
        bk @ (Backend::PhpFpm { .. } | Backend::PhpFpmTcp { .. }) => {
            fcgi::forward(req, bk, document_root, server_addr, peer_addr, https).await
        }
    }
}

fn bad_request_response() -> Response<BoxBody> {
    Response::builder()
        .status(StatusCode::BAD_REQUEST)
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(bytes_body(b"Missing or invalid Host header.\n"))
        .unwrap_or_else(|_| Response::new(empty_body()))
}

fn not_found_response() -> Response<BoxBody> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(bytes_body(b"No site matches this Host.\n"))
        .unwrap_or_else(|_| Response::new(empty_body()))
}

fn internal_error_response() -> Response<BoxBody> {
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(bytes_body(b"Proxy internal error.\n"))
        .unwrap_or_else(|_| Response::new(empty_body()))
}
