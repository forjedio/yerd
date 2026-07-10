//! HTTP-only integration test: drive `ProxyServer::serve` against a fake
//! FastCGI listener and a hyper client. Asserts the routing + CGI param
//! flow end-to-end.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::doc_markdown
)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use http_body_util::{BodyExt, Empty};
use hyper::Request;
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;

use yerd_core::{PhpVersion, RouterConfig, Site, SiteRouter, Tld};
use yerd_proxy::{Backend, BackendResolver, ProxyClientTls, ProxyError, ProxyServer};

/// A client-TLS bundle for tests: both configs accept any certificate (tests
/// only reach loopback upstreams).
fn test_client_tls() -> Arc<ProxyClientTls> {
    let local = ProxyClientTls::no_verify_config().unwrap();
    let public = ProxyClientTls::no_verify_config().unwrap();
    Arc::new(ProxyClientTls::new(local, public))
}

// ─── Test resolver ──────────────────────────────────────────────────

struct StaticResolver {
    backend: Backend,
}

#[async_trait]
impl BackendResolver for StaticResolver {
    async fn backend_for(&self, _site: &Site) -> Result<Backend, ProxyError> {
        Ok(self.backend.clone())
    }

    /// These tests predate the `WordPress`-only `resolve_script` gate and
    /// exercise plenty of scenarios that rely on direct script execution
    /// being on (e.g. `subdirectory_index_php_wins_over_root_index_php`), so
    /// this stub opts every site into it. The gate itself is proven by
    /// `direct_script_execution_gated_to_wordpress_sites` below.
    async fn allows_direct_script_execution(&self, _site: &Site) -> bool {
        true
    }
}

/// Resolver stub for `direct_script_execution_gated_to_wordpress_sites`:
/// resolves a backend like [`StaticResolver`] but leaves
/// `allows_direct_script_execution` at the trait's safe `false` default, to
/// prove a non-`WordPress` site never gets direct script execution.
struct NonWordPressResolver {
    backend: Backend,
}

#[async_trait]
impl BackendResolver for NonWordPressResolver {
    async fn backend_for(&self, _site: &Site) -> Result<Backend, ProxyError> {
        Ok(self.backend.clone())
    }
}

// ─── Cert store stub (unused on HTTP path) ──────────────────────────

#[derive(Debug)]
struct StubCertStore;
impl yerd_proxy::CertStore for StubCertStore {
    fn certified_key(&self, _: &str) -> Option<Arc<rustls::sign::CertifiedKey>> {
        None
    }
}

// ─── Login-token stub (one-click WP Admin login isn't exercised here) ──

struct NoLoginTokens;
impl yerd_proxy::LoginTokenConsumer for NoLoginTokens {
    fn consume(&self, _site: &str, _token: &str) -> Option<String> {
        None
    }
}

/// Valid for exactly one (site, token) pair, and only once - mirrors the
/// real `LoginTokenRegistry`'s single-use semantics closely enough to test
/// `dispatch`'s interception branch without pulling in the daemon crate.
struct OneShotLoginToken {
    site: &'static str,
    token: &'static str,
    target_user: &'static str,
    consumed: std::sync::atomic::AtomicBool,
}
impl yerd_proxy::LoginTokenConsumer for OneShotLoginToken {
    fn consume(&self, site: &str, token: &str) -> Option<String> {
        if self
            .consumed
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            return None;
        }
        (site == self.site && token == self.token).then(|| self.target_user.to_owned())
    }
}

// ─── Fake FastCGI listener ──────────────────────────────────────────

/// Accept exactly one connection; parse records; respond with the
/// canned stdout payload.
async fn run_fake_fcgi(
    listener: TcpListener,
    stdout_payload: Vec<u8>,
    captured_params: Arc<tokio::sync::Mutex<HashMap<String, String>>>,
) {
    let (mut conn, _) = listener.accept().await.unwrap();
    let mut params_buf: Vec<u8> = Vec::new();
    loop {
        let mut header = [0u8; 8];
        if conn.read_exact(&mut header).await.is_err() {
            break;
        }
        let record_type = header[1];
        let content_len = u16::from_be_bytes([header[4], header[5]]) as usize;
        let padding = header[6] as usize;
        let mut content = vec![0u8; content_len];
        if content_len > 0 {
            conn.read_exact(&mut content).await.unwrap();
        }
        if padding > 0 {
            let mut pad = vec![0u8; padding];
            conn.read_exact(&mut pad).await.unwrap();
        }
        // record types: 4 = PARAMS, 5 = STDIN
        if record_type == 4 {
            if content.is_empty() {
            } else {
                params_buf.extend_from_slice(&content);
            }
        } else if record_type == 5 && content.is_empty() {
            break;
        }
    }

    let parsed = decode_params(&params_buf);
    {
        let mut guard = captured_params.lock().await;
        *guard = parsed;
    }

    write_record(&mut conn, 6 /* STDOUT */, &stdout_payload).await;
    write_record(&mut conn, 6 /* STDOUT */, &[]).await;
    write_record(
        &mut conn,
        3, /* END_REQUEST */
        &[0, 0, 0, 0, 0, 0, 0, 0],
    )
    .await;
    let _ = conn.shutdown().await;
}

async fn write_record(conn: &mut TcpStream, record_type: u8, content: &[u8]) {
    let len = u16::try_from(content.len()).unwrap();
    let header: [u8; 8] = [
        1, // version
        record_type,
        0,
        1, // request_id = 1
        (len >> 8) as u8,
        (len & 0xFF) as u8,
        0,
        0, // padding + reserved
    ];
    conn.write_all(&header).await.unwrap();
    if !content.is_empty() {
        conn.write_all(content).await.unwrap();
    }
}

fn decode_params(buf: &[u8]) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut idx = 0;
    while idx < buf.len() {
        let (name_len, used) = read_len(&buf[idx..]);
        idx += used;
        let (value_len, used) = read_len(&buf[idx..]);
        idx += used;
        let name = String::from_utf8_lossy(&buf[idx..idx + name_len]).to_string();
        idx += name_len;
        let value = String::from_utf8_lossy(&buf[idx..idx + value_len]).to_string();
        idx += value_len;
        out.insert(name, value);
    }
    out
}

fn read_len(buf: &[u8]) -> (usize, usize) {
    if buf[0] & 0x80 == 0 {
        (buf[0] as usize, 1)
    } else {
        let v = u32::from_be_bytes([buf[0] & 0x7F, buf[1], buf[2], buf[3]]);
        (v as usize, 4)
    }
}

// ─── Test ───────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proxy_forwards_to_fcgi_backend() {
    let fcgi_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fcgi_addr = fcgi_listener.local_addr().unwrap();
    let captured = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let captured_for_fake = captured.clone();
    let stdout_payload = b"Status: 200 OK\r\nContent-Type: text/plain\r\n\r\nhello".to_vec();
    let fake_task = tokio::spawn(run_fake_fcgi(
        fcgi_listener,
        stdout_payload,
        captured_for_fake,
    ));

    let proxy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_addr = proxy_listener.local_addr().unwrap();

    let tld = Tld::new("test").unwrap();
    let cfg = RouterConfig::with_tld(tld);
    let mut router = SiteRouter::new(cfg);
    let site = Site::linked("app", PathBuf::from("/srv/www/app"), PhpVersion::new(8, 3)).unwrap();
    router.insert(site).unwrap();
    let router = Arc::new(tokio::sync::RwLock::new(router));

    let resolver = Arc::new(StaticResolver {
        backend: Backend::PhpFpmTcp { addr: fcgi_addr },
    });

    let (tx_shutdown, rx_shutdown) = oneshot::channel::<()>();
    let proxy_task = tokio::spawn(async move {
        let _ = ProxyServer::serve::<_, StubCertStore, _, _>(
            proxy_listener,
            None,
            router,
            resolver,
            Arc::new(NoLoginTokens),
            None,
            Arc::new(AtomicBool::new(true)),
            test_client_tls(),
            async move {
                let _ = rx_shutdown.await;
            },
        )
        .await;
    });

    let response_body = client_get(proxy_addr, "app.test", "/foo?bar=1").await;
    assert_eq!(response_body, b"hello");

    let params = captured.lock().await.clone();
    assert_eq!(
        params.get("REQUEST_METHOD").map(String::as_str),
        Some("GET")
    );
    assert_eq!(
        params.get("REQUEST_URI").map(String::as_str),
        Some("/foo?bar=1")
    );
    assert_eq!(
        params.get("SCRIPT_NAME").map(String::as_str),
        Some("/index.php")
    );
    assert!(params
        .get("SCRIPT_FILENAME")
        .unwrap()
        .ends_with("/index.php"));
    assert_eq!(params.get("PATH_INFO").map(String::as_str), Some("/foo"));
    assert_eq!(
        params.get("QUERY_STRING").map(String::as_str),
        Some("bar=1")
    );
    assert_eq!(
        params.get("SERVER_NAME").map(String::as_str),
        Some("app.test")
    );

    let _ = tx_shutdown.send(());
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), proxy_task).await;
    let _ = fake_task.await;
}

/// A valid one-click WordPress login token on `/wp-admin/`: the forwarded
/// request must carry `PHP_VALUE: auto_prepend_file=...`, and the token must
/// be gone from `QUERY_STRING`/`REQUEST_URI` - never reaching PHP or logging.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn valid_login_token_adds_auto_prepend_and_strips_token_from_query() {
    let fcgi_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fcgi_addr = fcgi_listener.local_addr().unwrap();
    let captured = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let captured_for_fake = captured.clone();
    let stdout_payload = b"Status: 200 OK\r\nContent-Type: text/plain\r\n\r\nadmin".to_vec();
    let fake_task = tokio::spawn(run_fake_fcgi(
        fcgi_listener,
        stdout_payload,
        captured_for_fake,
    ));

    let proxy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_addr = proxy_listener.local_addr().unwrap();

    let tld = Tld::new("test").unwrap();
    let cfg = RouterConfig::with_tld(tld);
    let mut router = SiteRouter::new(cfg);
    let site = Site::linked(
        "blog",
        PathBuf::from("/srv/www/blog"),
        PhpVersion::new(8, 3),
    )
    .unwrap();
    router.insert(site).unwrap();
    let router = Arc::new(tokio::sync::RwLock::new(router));

    let resolver = Arc::new(StaticResolver {
        backend: Backend::PhpFpmTcp { addr: fcgi_addr },
    });
    let login_tokens = Arc::new(OneShotLoginToken {
        site: "blog",
        token: "sekrit",
        target_user: "editor",
        consumed: std::sync::atomic::AtomicBool::new(false),
    });
    let prepend_path = PathBuf::from("/opt/yerd/wordpress-autologin-prepend.php");

    let (tx_shutdown, rx_shutdown) = oneshot::channel::<()>();
    let proxy_task = tokio::spawn(async move {
        let _ = ProxyServer::serve::<_, StubCertStore, _, _>(
            proxy_listener,
            None,
            router,
            resolver,
            login_tokens,
            Some(prepend_path),
            Arc::new(AtomicBool::new(true)),
            test_client_tls(),
            async move {
                let _ = rx_shutdown.await;
            },
        )
        .await;
    });

    let response_body = client_get(
        proxy_addr,
        "blog.test",
        "/wp-admin/?yerd_login_token=sekrit",
    )
    .await;
    assert_eq!(response_body, b"admin");

    let params = captured.lock().await.clone();
    assert_eq!(
        params.get("PHP_VALUE").map(String::as_str),
        Some("auto_prepend_file=/opt/yerd/wordpress-autologin-prepend.php")
    );
    assert_eq!(
        params.get("YERD_LOGIN_USER").map(String::as_str),
        Some("editor")
    );
    // The token must never reach PHP: stripped from both REQUEST_URI and
    // QUERY_STRING, and no dangling `?` or `&` left behind.
    assert_eq!(
        params.get("REQUEST_URI").map(String::as_str),
        Some("/wp-admin/")
    );
    assert_eq!(params.get("QUERY_STRING").map(String::as_str), Some(""));

    let _ = tx_shutdown.send(());
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), proxy_task).await;
    let _ = fake_task.await;
}

/// Ordering invariant: for a secure site presenting a login token over
/// plain HTTP, the HTTP->HTTPS redirect must happen *before* the token is
/// ever looked at, so a secure site's token is never burned by the 301
/// itself (see `dispatch`'s comment on `consume_login_token_if_present`'s
/// call site). Every other login-token test uses a non-secure site, so this
/// ordering was previously unverified.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn secure_site_redirect_does_not_consume_login_token() {
    let proxy_http_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_http_addr = proxy_http_listener.local_addr().unwrap();
    let https_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();

    let tld = Tld::new("test").unwrap();
    let cfg = RouterConfig::with_tld(tld);
    let mut router = SiteRouter::new(cfg);
    let mut site = Site::linked(
        "blog",
        PathBuf::from("/srv/www/blog"),
        PhpVersion::new(8, 3),
    )
    .unwrap();
    site.set_secure(true);
    router.insert(site).unwrap();
    let router = Arc::new(tokio::sync::RwLock::new(router));

    let resolver = Arc::new(StaticResolver {
        backend: Backend::PhpFpmTcp {
            addr: "127.0.0.1:1".parse().unwrap(),
        },
    });
    let login_tokens = Arc::new(OneShotLoginToken {
        site: "blog",
        token: "sekrit",
        target_user: "editor",
        consumed: std::sync::atomic::AtomicBool::new(false),
    });
    let login_tokens_for_assert = login_tokens.clone();

    let https = yerd_proxy::HttpsBinding {
        listener: https_listener,
        public_port: Arc::new(std::sync::atomic::AtomicU16::new(8443)),
        cert_store: Arc::new(StubCertStore),
    };

    let (tx_shutdown, rx_shutdown) = oneshot::channel::<()>();
    let proxy_task = tokio::spawn(async move {
        let _ = ProxyServer::serve(
            proxy_http_listener,
            Some(https),
            router,
            resolver,
            login_tokens,
            None,
            Arc::new(AtomicBool::new(true)),
            test_client_tls(),
            async move {
                let _ = rx_shutdown.await;
            },
        )
        .await;
    });

    let (status, location) = client_get_status_and_location(
        proxy_http_addr,
        "blog.test",
        "/wp-admin/?yerd_login_token=sekrit",
    )
    .await;
    assert_eq!(status, 301);
    assert_eq!(
        location.as_deref(),
        Some("https://blog.test:8443/wp-admin/?yerd_login_token=sekrit"),
        "the token must still be in the redirect Location, untouched"
    );
    assert!(
        !login_tokens_for_assert
            .consumed
            .load(std::sync::atomic::Ordering::SeqCst),
        "the 301 redirect must never consume the token"
    );

    let _ = tx_shutdown.send(());
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), proxy_task).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn unknown_host_returns_404() {
    let proxy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_addr = proxy_listener.local_addr().unwrap();

    let tld = Tld::new("test").unwrap();
    let cfg = RouterConfig::with_tld(tld);
    let router = Arc::new(tokio::sync::RwLock::new(SiteRouter::new(cfg)));
    let resolver = Arc::new(StaticResolver {
        backend: Backend::PhpFpmTcp {
            addr: "127.0.0.1:1".parse().unwrap(),
        },
    });

    let (tx_shutdown, rx_shutdown) = oneshot::channel::<()>();
    let proxy_task = tokio::spawn(async move {
        let _ = ProxyServer::serve::<_, StubCertStore, _, _>(
            proxy_listener,
            None,
            router,
            resolver,
            Arc::new(NoLoginTokens),
            None,
            Arc::new(AtomicBool::new(true)),
            test_client_tls(),
            async move {
                let _ = rx_shutdown.await;
            },
        )
        .await;
    });

    let status = client_get_status(proxy_addr, "missing.test", "/").await;
    assert_eq!(status, 404);

    let _ = tx_shutdown.send(());
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), proxy_task).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn missing_host_header_returns_400() {
    let proxy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_addr = proxy_listener.local_addr().unwrap();
    let tld = Tld::new("test").unwrap();
    let cfg = RouterConfig::with_tld(tld);
    let router = Arc::new(tokio::sync::RwLock::new(SiteRouter::new(cfg)));
    let resolver = Arc::new(StaticResolver {
        backend: Backend::PhpFpmTcp {
            addr: "127.0.0.1:1".parse().unwrap(),
        },
    });

    let (tx_shutdown, rx_shutdown) = oneshot::channel::<()>();
    let proxy_task = tokio::spawn(async move {
        let _ = ProxyServer::serve::<_, StubCertStore, _, _>(
            proxy_listener,
            None,
            router,
            resolver,
            Arc::new(NoLoginTokens),
            None,
            Arc::new(AtomicBool::new(true)),
            test_client_tls(),
            async move {
                let _ = rx_shutdown.await;
            },
        )
        .await;
    });

    let mut s = TcpStream::connect(proxy_addr).await.unwrap();
    s.write_all(b"GET / HTTP/1.1\r\n\r\n").await.unwrap();
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), s.read_to_end(&mut buf)).await;
    let resp = String::from_utf8_lossy(&buf);
    assert!(resp.contains("400"), "expected 400, got: {resp}");

    let _ = tx_shutdown.send(());
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), proxy_task).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn static_file_is_served_without_touching_fcgi() {
    let docroot = tempfile::tempdir().unwrap();
    let favicon = b"\x00\x00\x01\x00 fake-ico-bytes";
    std::fs::write(docroot.path().join("favicon.ico"), favicon).unwrap();

    let proxy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_addr = proxy_listener.local_addr().unwrap();

    let tld = Tld::new("test").unwrap();
    let cfg = RouterConfig::with_tld(tld);
    let mut router = SiteRouter::new(cfg);
    let site = Site::linked("app", docroot.path().to_path_buf(), PhpVersion::new(8, 3)).unwrap();
    router.insert(site).unwrap();
    let router = Arc::new(tokio::sync::RwLock::new(router));

    let resolver = Arc::new(StaticResolver {
        backend: Backend::PhpFpmTcp {
            addr: "127.0.0.1:1".parse().unwrap(),
        },
    });

    let (tx_shutdown, rx_shutdown) = oneshot::channel::<()>();
    let proxy_task = tokio::spawn(async move {
        let _ = ProxyServer::serve::<_, StubCertStore, _, _>(
            proxy_listener,
            None,
            router,
            resolver,
            Arc::new(NoLoginTokens),
            None,
            Arc::new(AtomicBool::new(true)),
            test_client_tls(),
            async move {
                let _ = rx_shutdown.await;
            },
        )
        .await;
    });

    let (status, content_type, body) =
        client_get_response(proxy_addr, "app.test", "/favicon.ico").await;
    assert_eq!(status, 200);
    assert_eq!(content_type.as_deref(), Some("image/x-icon"));
    assert_eq!(body, favicon);

    let _ = tx_shutdown.send(());
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), proxy_task).await;
}

/// The configured FastCGI backend (`127.0.0.1:1`) is deliberately
/// unreachable: if the request ever fell through to `fcgi::forward` it
/// would hard-fail, so a 200 here proves the directory index short-circuited
/// the front-controller path.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn directory_index_html_served_when_no_index_php() {
    let docroot = tempfile::tempdir().unwrap();
    std::fs::write(docroot.path().join("index.html"), b"<h1>static site</h1>").unwrap();

    let proxy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_addr = proxy_listener.local_addr().unwrap();

    let tld = Tld::new("test").unwrap();
    let cfg = RouterConfig::with_tld(tld);
    let mut router = SiteRouter::new(cfg);
    let site = Site::linked("app", docroot.path().to_path_buf(), PhpVersion::new(8, 3)).unwrap();
    router.insert(site).unwrap();
    let router = Arc::new(tokio::sync::RwLock::new(router));

    let resolver = Arc::new(StaticResolver {
        backend: Backend::PhpFpmTcp {
            addr: "127.0.0.1:1".parse().unwrap(),
        },
    });

    let (tx_shutdown, rx_shutdown) = oneshot::channel::<()>();
    let proxy_task = tokio::spawn(async move {
        let _ = ProxyServer::serve::<_, StubCertStore, _, _>(
            proxy_listener,
            None,
            router,
            resolver,
            Arc::new(NoLoginTokens),
            None,
            Arc::new(AtomicBool::new(true)),
            test_client_tls(),
            async move {
                let _ = rx_shutdown.await;
            },
        )
        .await;
    });

    let (status, content_type, body) = client_get_response(proxy_addr, "app.test", "/").await;
    assert_eq!(status, 200);
    assert_eq!(content_type.as_deref(), Some("text/html; charset=utf-8"));
    assert_eq!(body, b"<h1>static site</h1>");

    let _ = tx_shutdown.send(());
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), proxy_task).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn directory_index_htm_served_as_fallback() {
    let docroot = tempfile::tempdir().unwrap();
    std::fs::create_dir(docroot.path().join("blog")).unwrap();
    std::fs::write(docroot.path().join("blog/index.htm"), b"blog home").unwrap();

    let proxy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_addr = proxy_listener.local_addr().unwrap();

    let tld = Tld::new("test").unwrap();
    let cfg = RouterConfig::with_tld(tld);
    let mut router = SiteRouter::new(cfg);
    let site = Site::linked("app", docroot.path().to_path_buf(), PhpVersion::new(8, 3)).unwrap();
    router.insert(site).unwrap();
    let router = Arc::new(tokio::sync::RwLock::new(router));

    let resolver = Arc::new(StaticResolver {
        backend: Backend::PhpFpmTcp {
            addr: "127.0.0.1:1".parse().unwrap(),
        },
    });

    let (tx_shutdown, rx_shutdown) = oneshot::channel::<()>();
    let proxy_task = tokio::spawn(async move {
        let _ = ProxyServer::serve::<_, StubCertStore, _, _>(
            proxy_listener,
            None,
            router,
            resolver,
            Arc::new(NoLoginTokens),
            None,
            Arc::new(AtomicBool::new(true)),
            test_client_tls(),
            async move {
                let _ = rx_shutdown.await;
            },
        )
        .await;
    });

    let (status, content_type, body) = client_get_response(proxy_addr, "app.test", "/blog/").await;
    assert_eq!(status, 200);
    assert_eq!(content_type.as_deref(), Some("text/html; charset=utf-8"));
    assert_eq!(body, b"blog home");

    let _ = tx_shutdown.send(());
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), proxy_task).await;
}

/// The response body alone can't distinguish "correctly deferred to the
/// front controller" from "the fix is entirely absent" - both forward to
/// FastCGI and get the same canned reply. The assertion on `SCRIPT_NAME`
/// below is what proves the request actually reached `index.php`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn index_php_present_wins_over_index_html() {
    let docroot = tempfile::tempdir().unwrap();
    std::fs::write(docroot.path().join("index.php"), b"<?php ?>").unwrap();
    std::fs::write(docroot.path().join("index.html"), b"should not be served").unwrap();

    let fcgi_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fcgi_addr = fcgi_listener.local_addr().unwrap();
    let captured = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let captured_for_fake = captured.clone();
    let stdout_payload = b"Status: 200 OK\r\nContent-Type: text/plain\r\n\r\nfrom fpm".to_vec();
    let fake_task = tokio::spawn(run_fake_fcgi(
        fcgi_listener,
        stdout_payload,
        captured_for_fake,
    ));

    let proxy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_addr = proxy_listener.local_addr().unwrap();

    let tld = Tld::new("test").unwrap();
    let cfg = RouterConfig::with_tld(tld);
    let mut router = SiteRouter::new(cfg);
    let site = Site::linked("app", docroot.path().to_path_buf(), PhpVersion::new(8, 3)).unwrap();
    router.insert(site).unwrap();
    let router = Arc::new(tokio::sync::RwLock::new(router));

    let resolver = Arc::new(StaticResolver {
        backend: Backend::PhpFpmTcp { addr: fcgi_addr },
    });

    let (tx_shutdown, rx_shutdown) = oneshot::channel::<()>();
    let proxy_task = tokio::spawn(async move {
        let _ = ProxyServer::serve::<_, StubCertStore, _, _>(
            proxy_listener,
            None,
            router,
            resolver,
            Arc::new(NoLoginTokens),
            None,
            Arc::new(AtomicBool::new(true)),
            test_client_tls(),
            async move {
                let _ = rx_shutdown.await;
            },
        )
        .await;
    });

    let body = client_get(proxy_addr, "app.test", "/").await;
    assert_eq!(body, b"from fpm");

    let params = captured.lock().await.clone();
    assert_eq!(
        params.get("SCRIPT_NAME").map(String::as_str),
        Some("/index.php")
    );

    let _ = tx_shutdown.send(());
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), proxy_task).await;
    let _ = fake_task.await;
}

/// The exact WordPress `/wp-admin/` bug report: a real subdirectory script
/// (`wp-admin/index.php`) must execute directly, not the site root's own
/// `index.php` - a request for a specific admin/login/cron entry point must
/// never silently render the front page instead.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subdirectory_index_php_wins_over_root_index_php() {
    let docroot = tempfile::tempdir().unwrap();
    std::fs::write(docroot.path().join("index.php"), b"<?php /* front page */").unwrap();
    std::fs::create_dir(docroot.path().join("wp-admin")).unwrap();
    std::fs::write(
        docroot.path().join("wp-admin/index.php"),
        b"<?php /* wp-admin */",
    )
    .unwrap();

    let fcgi_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fcgi_addr = fcgi_listener.local_addr().unwrap();
    let captured = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let captured_for_fake = captured.clone();
    let stdout_payload = b"Status: 200 OK\r\nContent-Type: text/plain\r\n\r\nfrom fpm".to_vec();
    let fake_task = tokio::spawn(run_fake_fcgi(
        fcgi_listener,
        stdout_payload,
        captured_for_fake,
    ));

    let proxy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_addr = proxy_listener.local_addr().unwrap();

    let tld = Tld::new("test").unwrap();
    let cfg = RouterConfig::with_tld(tld);
    let mut router = SiteRouter::new(cfg);
    let site = Site::linked("blog", docroot.path().to_path_buf(), PhpVersion::new(8, 3)).unwrap();
    router.insert(site).unwrap();
    let router = Arc::new(tokio::sync::RwLock::new(router));

    let resolver = Arc::new(StaticResolver {
        backend: Backend::PhpFpmTcp { addr: fcgi_addr },
    });

    let (tx_shutdown, rx_shutdown) = oneshot::channel::<()>();
    let proxy_task = tokio::spawn(async move {
        let _ = ProxyServer::serve::<_, StubCertStore, _, _>(
            proxy_listener,
            None,
            router,
            resolver,
            Arc::new(NoLoginTokens),
            None,
            Arc::new(AtomicBool::new(true)),
            test_client_tls(),
            async move {
                let _ = rx_shutdown.await;
            },
        )
        .await;
    });

    let body = client_get(proxy_addr, "blog.test", "/wp-admin/").await;
    assert_eq!(body, b"from fpm");

    let params = captured.lock().await.clone();
    assert_eq!(
        params.get("SCRIPT_NAME").map(String::as_str),
        Some("/wp-admin/index.php")
    );
    assert_eq!(
        params.get("SCRIPT_FILENAME").map(String::as_str),
        Some(docroot.path().join("wp-admin/index.php").to_str().unwrap())
    );

    let _ = tx_shutdown.send(());
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), proxy_task).await;
    let _ = fake_task.await;
}

/// A non-`WordPress` site must never get `resolve_script`'s direct-real-
/// file-execution treatment: a stray real script under the document root
/// (a debug `phpinfo.php`, an old admin tool) stays unreachable directly and
/// every request still funnels through the site root's `index.php`, exactly
/// as it did before the `WordPress` front-controller policy existed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn direct_script_execution_gated_to_wordpress_sites() {
    let docroot = tempfile::tempdir().unwrap();
    std::fs::write(docroot.path().join("index.php"), b"<?php /* front page */").unwrap();
    std::fs::write(docroot.path().join("phpinfo.php"), b"<?php phpinfo();").unwrap();

    let fcgi_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fcgi_addr = fcgi_listener.local_addr().unwrap();
    let captured = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let captured_for_fake = captured.clone();
    let stdout_payload = b"Status: 200 OK\r\nContent-Type: text/plain\r\n\r\nfrom fpm".to_vec();
    let fake_task = tokio::spawn(run_fake_fcgi(
        fcgi_listener,
        stdout_payload,
        captured_for_fake,
    ));

    let proxy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_addr = proxy_listener.local_addr().unwrap();

    let tld = Tld::new("test").unwrap();
    let cfg = RouterConfig::with_tld(tld);
    let mut router = SiteRouter::new(cfg);
    let site = Site::linked("shop", docroot.path().to_path_buf(), PhpVersion::new(8, 3)).unwrap();
    router.insert(site).unwrap();
    let router = Arc::new(tokio::sync::RwLock::new(router));

    let resolver = Arc::new(NonWordPressResolver {
        backend: Backend::PhpFpmTcp { addr: fcgi_addr },
    });

    let (tx_shutdown, rx_shutdown) = oneshot::channel::<()>();
    let proxy_task = tokio::spawn(async move {
        let _ = ProxyServer::serve::<_, StubCertStore, _, _>(
            proxy_listener,
            None,
            router,
            resolver,
            Arc::new(NoLoginTokens),
            None,
            Arc::new(AtomicBool::new(true)),
            test_client_tls(),
            async move {
                let _ = rx_shutdown.await;
            },
        )
        .await;
    });

    let body = client_get(proxy_addr, "shop.test", "/phpinfo.php").await;
    assert_eq!(body, b"from fpm");

    let params = captured.lock().await.clone();
    assert_eq!(
        params.get("SCRIPT_NAME").map(String::as_str),
        Some("/index.php")
    );
    assert_eq!(
        params.get("SCRIPT_FILENAME").map(String::as_str),
        Some(docroot.path().join("index.php").to_str().unwrap())
    );

    let _ = tx_shutdown.send(());
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), proxy_task).await;
    let _ = fake_task.await;
}

/// A real directory with none of index.php/html/htm must still reach the
/// front controller rather than dead-ending in a 404.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn directory_with_no_index_at_all_falls_through_to_fcgi() {
    let docroot = tempfile::tempdir().unwrap();
    std::fs::create_dir(docroot.path().join("empty")).unwrap();

    let fcgi_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fcgi_addr = fcgi_listener.local_addr().unwrap();
    let captured = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let captured_for_fake = captured.clone();
    let stdout_payload = b"Status: 200 OK\r\nContent-Type: text/plain\r\n\r\nfrom fpm".to_vec();
    let fake_task = tokio::spawn(run_fake_fcgi(
        fcgi_listener,
        stdout_payload,
        captured_for_fake,
    ));

    let proxy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_addr = proxy_listener.local_addr().unwrap();

    let tld = Tld::new("test").unwrap();
    let cfg = RouterConfig::with_tld(tld);
    let mut router = SiteRouter::new(cfg);
    let site = Site::linked("app", docroot.path().to_path_buf(), PhpVersion::new(8, 3)).unwrap();
    router.insert(site).unwrap();
    let router = Arc::new(tokio::sync::RwLock::new(router));

    let resolver = Arc::new(StaticResolver {
        backend: Backend::PhpFpmTcp { addr: fcgi_addr },
    });

    let (tx_shutdown, rx_shutdown) = oneshot::channel::<()>();
    let proxy_task = tokio::spawn(async move {
        let _ = ProxyServer::serve::<_, StubCertStore, _, _>(
            proxy_listener,
            None,
            router,
            resolver,
            Arc::new(NoLoginTokens),
            None,
            Arc::new(AtomicBool::new(true)),
            test_client_tls(),
            async move {
                let _ = rx_shutdown.await;
            },
        )
        .await;
    });

    let body = client_get(proxy_addr, "app.test", "/empty/").await;
    assert_eq!(body, b"from fpm");
    assert_eq!(
        captured.lock().await.get("SCRIPT_NAME").map(String::as_str),
        Some("/index.php")
    );

    let _ = tx_shutdown.send(());
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), proxy_task).await;
    let _ = fake_task.await;
}

/// Covers the trailing-slash pretty-URL framework route (e.g.
/// `/blog/some-post/`) where nothing on disk matches the path:
/// `canonicalize()` fails, and the request must still reach `index.php`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nonexistent_directory_falls_through_to_fcgi() {
    let docroot = tempfile::tempdir().unwrap();

    let fcgi_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fcgi_addr = fcgi_listener.local_addr().unwrap();
    let captured = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let captured_for_fake = captured.clone();
    let stdout_payload = b"Status: 200 OK\r\nContent-Type: text/plain\r\n\r\nfrom fpm".to_vec();
    let fake_task = tokio::spawn(run_fake_fcgi(
        fcgi_listener,
        stdout_payload,
        captured_for_fake,
    ));

    let proxy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_addr = proxy_listener.local_addr().unwrap();

    let tld = Tld::new("test").unwrap();
    let cfg = RouterConfig::with_tld(tld);
    let mut router = SiteRouter::new(cfg);
    let site = Site::linked("app", docroot.path().to_path_buf(), PhpVersion::new(8, 3)).unwrap();
    router.insert(site).unwrap();
    let router = Arc::new(tokio::sync::RwLock::new(router));

    let resolver = Arc::new(StaticResolver {
        backend: Backend::PhpFpmTcp { addr: fcgi_addr },
    });

    let (tx_shutdown, rx_shutdown) = oneshot::channel::<()>();
    let proxy_task = tokio::spawn(async move {
        let _ = ProxyServer::serve::<_, StubCertStore, _, _>(
            proxy_listener,
            None,
            router,
            resolver,
            Arc::new(NoLoginTokens),
            None,
            Arc::new(AtomicBool::new(true)),
            test_client_tls(),
            async move {
                let _ = rx_shutdown.await;
            },
        )
        .await;
    });

    let body = client_get(proxy_addr, "app.test", "/blog/some-post/").await;
    assert_eq!(body, b"from fpm");
    assert_eq!(
        captured.lock().await.get("SCRIPT_NAME").map(String::as_str),
        Some("/index.php")
    );

    let _ = tx_shutdown.send(());
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), proxy_task).await;
    let _ = fake_task.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn head_request_to_directory_index_returns_empty_body() {
    let docroot = tempfile::tempdir().unwrap();
    std::fs::write(docroot.path().join("index.html"), b"<h1>hello</h1>").unwrap();

    let proxy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_addr = proxy_listener.local_addr().unwrap();

    let tld = Tld::new("test").unwrap();
    let cfg = RouterConfig::with_tld(tld);
    let mut router = SiteRouter::new(cfg);
    let site = Site::linked("app", docroot.path().to_path_buf(), PhpVersion::new(8, 3)).unwrap();
    router.insert(site).unwrap();
    let router = Arc::new(tokio::sync::RwLock::new(router));

    let resolver = Arc::new(StaticResolver {
        backend: Backend::PhpFpmTcp {
            addr: "127.0.0.1:1".parse().unwrap(),
        },
    });

    let (tx_shutdown, rx_shutdown) = oneshot::channel::<()>();
    let proxy_task = tokio::spawn(async move {
        let _ = ProxyServer::serve::<_, StubCertStore, _, _>(
            proxy_listener,
            None,
            router,
            resolver,
            Arc::new(NoLoginTokens),
            None,
            Arc::new(AtomicBool::new(true)),
            test_client_tls(),
            async move {
                let _ = rx_shutdown.await;
            },
        )
        .await;
    });

    let stream = TcpStream::connect(proxy_addr).await.unwrap();
    let io = TokioIo::new(stream);
    let (mut sender, conn) = hyper::client::conn::http1::handshake::<_, Empty<Bytes>>(io)
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let req = Request::builder()
        .method("HEAD")
        .uri("/")
        .header("Host", "app.test")
        .body(Empty::<Bytes>::new())
        .unwrap();
    let resp = sender.send_request(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()
            .get(hyper::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok()),
        Some("14")
    );
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    assert!(body.is_empty());

    let _ = tx_shutdown.send(());
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), proxy_task).await;
}

/// Regression test for the Laravel `public/storage -> ../storage/app/public`
/// shape: a symlink under the served root that points outside it, but stays
/// within the site's `document_root`, must be served normally rather than
/// rejected. Uses a relative symlink target, matching exactly what
/// `artisan storage:link` creates (as opposed to an absolute target).
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn symlink_within_document_root_outside_served_root_is_served() {
    let docroot = tempfile::tempdir().unwrap();
    let storage_dir = docroot.path().join("storage/app/public");
    std::fs::create_dir_all(&storage_dir).unwrap();
    std::fs::write(storage_dir.join("logo.png"), b"logo-bytes").unwrap();

    let public_dir = docroot.path().join("public");
    std::fs::create_dir_all(&public_dir).unwrap();
    std::os::unix::fs::symlink("../storage/app/public", public_dir.join("storage")).unwrap();

    let proxy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_addr = proxy_listener.local_addr().unwrap();

    let tld = Tld::new("test").unwrap();
    let cfg = RouterConfig::with_tld(tld);
    let mut router = SiteRouter::new(cfg);
    let mut site =
        Site::linked("app", docroot.path().to_path_buf(), PhpVersion::new(8, 3)).unwrap();
    site.set_web_subpath("public");
    router.insert(site).unwrap();
    let router = Arc::new(tokio::sync::RwLock::new(router));

    let resolver = Arc::new(StaticResolver {
        backend: Backend::PhpFpmTcp {
            addr: "127.0.0.1:1".parse().unwrap(),
        },
    });

    let (tx_shutdown, rx_shutdown) = oneshot::channel::<()>();
    let proxy_task = tokio::spawn(async move {
        let _ = ProxyServer::serve::<_, StubCertStore, _, _>(
            proxy_listener,
            None,
            router,
            resolver,
            Arc::new(NoLoginTokens),
            None,
            Arc::new(AtomicBool::new(true)),
            test_client_tls(),
            async move {
                let _ = rx_shutdown.await;
            },
        )
        .await;
    });

    let (status, _content_type, body) =
        client_get_response(proxy_addr, "app.test", "/storage/logo.png").await;
    assert_eq!(status, 200);
    assert_eq!(body, b"logo-bytes");

    let _ = tx_shutdown.send(());
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), proxy_task).await;
}

/// A symlink that escapes the site's `document_root` entirely still gets
/// rejected - but now with an explicit `403` from yerd-proxy naming only the
/// requested path (the resolved path and allowed root are logged, not echoed,
/// to avoid leaking local absolute paths), instead of a silent fallthrough to
/// FastCGI.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn symlink_escaping_document_root_returns_403() {
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.txt"), b"leaked-secret").unwrap();

    let docroot = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("secret.txt"),
        docroot.path().join("evil.txt"),
    )
    .unwrap();

    let proxy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_addr = proxy_listener.local_addr().unwrap();

    let tld = Tld::new("test").unwrap();
    let cfg = RouterConfig::with_tld(tld);
    let mut router = SiteRouter::new(cfg);
    let site = Site::linked("app", docroot.path().to_path_buf(), PhpVersion::new(8, 3)).unwrap();
    router.insert(site).unwrap();
    let router = Arc::new(tokio::sync::RwLock::new(router));

    let resolver = Arc::new(StaticResolver {
        backend: Backend::PhpFpmTcp {
            addr: "127.0.0.1:1".parse().unwrap(),
        },
    });

    let (tx_shutdown, rx_shutdown) = oneshot::channel::<()>();
    let proxy_task = tokio::spawn(async move {
        let _ = ProxyServer::serve::<_, StubCertStore, _, _>(
            proxy_listener,
            None,
            router,
            resolver,
            Arc::new(NoLoginTokens),
            None,
            Arc::new(AtomicBool::new(true)),
            test_client_tls(),
            async move {
                let _ = rx_shutdown.await;
            },
        )
        .await;
    });

    let (status, _content_type, body) =
        client_get_response(proxy_addr, "app.test", "/evil.txt").await;
    assert_eq!(status, 403);
    assert!(!body
        .windows(b"leaked-secret".len())
        .any(|w| w == b"leaked-secret"));
    let body_str = String::from_utf8_lossy(&body);
    assert!(body_str.contains("/evil.txt"));
    assert!(!body_str.contains(&docroot.path().to_string_lossy().into_owned()));

    let _ = tx_shutdown.send(());
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), proxy_task).await;
}

/// Issue #112: with `symlink_protection` off, an asset reached through a symlink
/// that resolves outside the site's document root (e.g. a shared theme kept
/// beside the site) is served normally instead of the `403` above - the
/// user-opt-out this setting exists for.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn symlink_escaping_document_root_is_served_when_protection_off() {
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("style.css"), b"shared-theme-css").unwrap();

    let docroot = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("style.css"),
        docroot.path().join("style.css"),
    )
    .unwrap();

    let proxy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_addr = proxy_listener.local_addr().unwrap();

    let tld = Tld::new("test").unwrap();
    let cfg = RouterConfig::with_tld(tld);
    let mut router = SiteRouter::new(cfg);
    let site = Site::linked("app", docroot.path().to_path_buf(), PhpVersion::new(8, 3)).unwrap();
    router.insert(site).unwrap();
    let router = Arc::new(tokio::sync::RwLock::new(router));

    let resolver = Arc::new(StaticResolver {
        backend: Backend::PhpFpmTcp {
            addr: "127.0.0.1:1".parse().unwrap(),
        },
    });

    let (tx_shutdown, rx_shutdown) = oneshot::channel::<()>();
    let proxy_task = tokio::spawn(async move {
        let _ = ProxyServer::serve::<_, StubCertStore, _, _>(
            proxy_listener,
            None,
            router,
            resolver,
            Arc::new(NoLoginTokens),
            None,
            Arc::new(AtomicBool::new(false)),
            test_client_tls(),
            async move {
                let _ = rx_shutdown.await;
            },
        )
        .await;
    });

    let (status, _content_type, body) =
        client_get_response(proxy_addr, "app.test", "/style.css").await;
    assert_eq!(status, 200);
    assert_eq!(body, b"shared-theme-css");

    let _ = tx_shutdown.send(());
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), proxy_task).await;
}

/// Regression test for a symlink-escape hole: `try_serve_index` used to
/// canonicalise only the *directory*, then serve `directory.join("index.html")`
/// without re-canonicalising the resolved file. A symlink named `index.html`
/// pointing outside the site's `document_root` (or at PHP source inside it)
/// was served verbatim as a 200 `text/html` response. It's now rejected with
/// an explicit `403 Forbidden` from yerd-proxy rather than a silent
/// fallthrough to FastCGI.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn symlinked_index_html_escaping_root_is_not_served() {
    let secret_dir = tempfile::tempdir().unwrap();
    let secret_path = secret_dir.path().join("secret.php");
    std::fs::write(&secret_path, b"<?php secret_credentials(); ?>").unwrap();

    let docroot = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(&secret_path, docroot.path().join("index.html")).unwrap();

    let proxy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_addr = proxy_listener.local_addr().unwrap();

    let tld = Tld::new("test").unwrap();
    let cfg = RouterConfig::with_tld(tld);
    let mut router = SiteRouter::new(cfg);
    let site = Site::linked("app", docroot.path().to_path_buf(), PhpVersion::new(8, 3)).unwrap();
    router.insert(site).unwrap();
    let router = Arc::new(tokio::sync::RwLock::new(router));

    let resolver = Arc::new(StaticResolver {
        backend: Backend::PhpFpmTcp {
            addr: "127.0.0.1:1".parse().unwrap(),
        },
    });

    let (tx_shutdown, rx_shutdown) = oneshot::channel::<()>();
    let proxy_task = tokio::spawn(async move {
        let _ = ProxyServer::serve::<_, StubCertStore, _, _>(
            proxy_listener,
            None,
            router,
            resolver,
            Arc::new(NoLoginTokens),
            None,
            Arc::new(AtomicBool::new(true)),
            test_client_tls(),
            async move {
                let _ = rx_shutdown.await;
            },
        )
        .await;
    });

    let (status, _content_type, body) = client_get_response(proxy_addr, "app.test", "/").await;
    assert_eq!(status, 403);
    assert!(!body
        .windows(b"secret_credentials".len())
        .any(|w| w == b"secret_credentials"));

    let _ = tx_shutdown.send(());
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), proxy_task).await;
}

// ─── Hyper client helpers ───────────────────────────────────────────

async fn client_get(addr: SocketAddr, host: &str, path: &str) -> Vec<u8> {
    let stream = TcpStream::connect(addr).await.unwrap();
    let io = TokioIo::new(stream);
    let (mut sender, conn) = hyper::client::conn::http1::handshake::<_, Empty<Bytes>>(io)
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let req = Request::builder()
        .method("GET")
        .uri(path)
        .header("Host", host)
        .body(Empty::<Bytes>::new())
        .unwrap();
    let resp = sender.send_request(req).await.unwrap();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    body.to_vec()
}

async fn client_get_status_and_location(
    addr: SocketAddr,
    host: &str,
    path: &str,
) -> (u16, Option<String>) {
    let stream = TcpStream::connect(addr).await.unwrap();
    let io = TokioIo::new(stream);
    let (mut sender, conn) = hyper::client::conn::http1::handshake::<_, Empty<Bytes>>(io)
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let req = Request::builder()
        .method("GET")
        .uri(path)
        .header("Host", host)
        .body(Empty::<Bytes>::new())
        .unwrap();
    let resp = sender.send_request(req).await.unwrap();
    let status = resp.status().as_u16();
    let location = resp
        .headers()
        .get(hyper::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    (status, location)
}

async fn client_get_response(
    addr: SocketAddr,
    host: &str,
    path: &str,
) -> (u16, Option<String>, Vec<u8>) {
    let stream = TcpStream::connect(addr).await.unwrap();
    let io = TokioIo::new(stream);
    let (mut sender, conn) = hyper::client::conn::http1::handshake::<_, Empty<Bytes>>(io)
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let req = Request::builder()
        .method("GET")
        .uri(path)
        .header("Host", host)
        .body(Empty::<Bytes>::new())
        .unwrap();
    let resp = sender.send_request(req).await.unwrap();
    let status = resp.status().as_u16();
    let content_type = resp
        .headers()
        .get(hyper::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let body = resp
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    (status, content_type, body)
}

async fn client_get_status(addr: SocketAddr, host: &str, path: &str) -> u16 {
    let stream = TcpStream::connect(addr).await.unwrap();
    let io = TokioIo::new(stream);
    let (mut sender, conn) = hyper::client::conn::http1::handshake::<_, Empty<Bytes>>(io)
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let req = Request::builder()
        .method("GET")
        .uri(path)
        .header("Host", host)
        .body(Empty::<Bytes>::new())
        .unwrap();
    let resp = sender.send_request(req).await.unwrap();
    resp.status().as_u16()
}
