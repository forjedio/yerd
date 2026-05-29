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
use yerd_proxy::{Backend, BackendResolver, ProxyError, ProxyServer};

// ─── Test resolver ──────────────────────────────────────────────────

struct StaticResolver {
    backend: Backend,
}

#[async_trait]
impl BackendResolver for StaticResolver {
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

// ─── Fake FastCGI listener ──────────────────────────────────────────

/// Accept exactly one connection; parse records; respond with the
/// canned stdout payload.
async fn run_fake_fcgi(
    listener: TcpListener,
    stdout_payload: Vec<u8>,
    captured_params: Arc<tokio::sync::Mutex<HashMap<String, String>>>,
) {
    let (mut conn, _) = listener.accept().await.unwrap();
    // Read all incoming records until STDIN terminator (empty STDIN record).
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
        // 4 = PARAMS, 5 = STDIN, 1 = BEGIN_REQUEST, 2 = ABORT_REQUEST, ...
        if record_type == 4 {
            if content.is_empty() {
                // PARAMS terminator.
            } else {
                params_buf.extend_from_slice(&content);
            }
        } else if record_type == 5 && content.is_empty() {
            // STDIN terminator — done reading.
            break;
        }
    }

    // Decode the collected params and stash them in the shared map.
    let parsed = decode_params(&params_buf);
    {
        let mut guard = captured_params.lock().await;
        *guard = parsed;
    }

    // STDOUT record + END_REQUEST.
    write_record(&mut conn, 6 /* STDOUT */, &stdout_payload).await;
    write_record(&mut conn, 6 /* STDOUT */, &[]).await; // STDOUT terminator (optional)
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
    // 1. Fake FCGI listener.
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

    // 2. Proxy listener (HTTP only).
    let proxy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_addr = proxy_listener.local_addr().unwrap();

    // 3. Router: one Site at app.test.
    let tld = Tld::new("test").unwrap();
    let cfg = RouterConfig::with_tld(tld);
    let mut router = SiteRouter::new(cfg);
    let site = Site::linked("app", PathBuf::from("/srv/www/app"), PhpVersion::new(8, 3)).unwrap();
    router.insert(site).unwrap();
    let router = Arc::new(tokio::sync::RwLock::new(router));

    // 4. Resolver.
    let resolver = Arc::new(StaticResolver {
        backend: Backend::PhpFpmTcp { addr: fcgi_addr },
    });

    // 5. Run proxy with a shutdown channel.
    let (tx_shutdown, rx_shutdown) = oneshot::channel::<()>();
    let proxy_task = tokio::spawn(async move {
        let _ = ProxyServer::serve::<_, StubCertStore, _>(
            proxy_listener,
            None,
            router,
            resolver,
            async move {
                let _ = rx_shutdown.await;
            },
        )
        .await;
    });

    // 6. Hyper client → proxy.
    let response_body = client_get(proxy_addr, "app.test", "/foo?bar=1").await;
    assert_eq!(response_body, b"hello");

    // 7. Verify captured params.
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
        let _ = ProxyServer::serve::<_, StubCertStore, _>(
            proxy_listener,
            None,
            router,
            resolver,
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
        let _ = ProxyServer::serve::<_, StubCertStore, _>(
            proxy_listener,
            None,
            router,
            resolver,
            async move {
                let _ = rx_shutdown.await;
            },
        )
        .await;
    });

    // Send a request without the Host header (manual TCP).
    let mut s = TcpStream::connect(proxy_addr).await.unwrap();
    s.write_all(b"GET / HTTP/1.1\r\n\r\n").await.unwrap();
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), s.read_to_end(&mut buf)).await;
    let resp = String::from_utf8_lossy(&buf);
    assert!(resp.contains("400"), "expected 400, got: {resp}");

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
