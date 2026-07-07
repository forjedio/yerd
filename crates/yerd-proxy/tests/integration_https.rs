//! HTTPS integration test: drive `ProxyServer::serve` with a real CA +
//! leaf cert from `yerd-tls`, send a request through a rustls hyper
//! client, and verify it reaches the fake FastCGI backend.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::doc_markdown,
    clippy::redundant_closure_for_method_calls
)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use http_body_util::{BodyExt, Empty};
use hyper::Request;
use hyper_util::rt::TokioIo;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use rustls::sign::CertifiedKey;
use rustls::{ClientConfig, RootCertStore};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;

use yerd_core::{PhpVersion, RouterConfig, Site, SiteRouter, Tld};
use yerd_proxy::{Backend, BackendResolver, CertStore, HttpsBinding, ProxyError, ProxyServer};
use yerd_tls::{CertAuthority, Validity};

// ─── Resolver ───────────────────────────────────────────────────────

struct StaticResolver {
    backend: Backend,
}

#[async_trait]
impl BackendResolver for StaticResolver {
    async fn backend_for(&self, _site: &Site) -> Result<Backend, ProxyError> {
        Ok(self.backend.clone())
    }
}

// ─── Login-token stub (one-click WP Admin login isn't exercised here) ──

struct NoLoginTokens;
impl yerd_proxy::LoginTokenConsumer for NoLoginTokens {
    fn consume(&self, _site: &str, _token: &str) -> Option<String> {
        None
    }
}

// ─── CertStore ──────────────────────────────────────────────────────

#[derive(Debug)]
struct OneCertStore {
    host: String,
    key: Arc<CertifiedKey>,
}

impl CertStore for OneCertStore {
    fn certified_key(&self, sni: &str) -> Option<Arc<CertifiedKey>> {
        if sni.eq_ignore_ascii_case(&self.host) {
            Some(Arc::clone(&self.key))
        } else {
            None
        }
    }
}

fn validity() -> Validity {
    let now = time::OffsetDateTime::now_utc();
    let nb = now - time::Duration::days(1);
    let na = now + time::Duration::days(365);
    Validity::new(nb, na).unwrap()
}

fn parse_certified(cert_pem: &str, key_pem: &str) -> CertifiedKey {
    let cert_der: Vec<CertificateDer<'static>> =
        CertificateDer::pem_slice_iter(cert_pem.as_bytes())
            .map(Result::unwrap)
            .map(|c| c.into_owned())
            .collect();
    assert!(!cert_der.is_empty(), "no certs parsed from PEM");
    let key_der: PrivateKeyDer<'static> = PrivateKeyDer::from_pem_slice(key_pem.as_bytes())
        .unwrap()
        .clone_key();
    yerd_proxy::tls::init_crypto_once();
    let signing_key = rustls::crypto::ring::sign::any_supported_type(&key_der).unwrap();
    CertifiedKey::new(cert_der, signing_key)
}

// ─── Fake FCGI (TCP) ────────────────────────────────────────────────

async fn run_fake_fcgi(listener: TcpListener, stdout_payload: Vec<u8>) {
    let (mut conn, _) = listener.accept().await.unwrap();
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
        if record_type == 5 && content.is_empty() {
            break;
        }
    }
    write_record(&mut conn, 6 /* STDOUT */, &stdout_payload).await;
    write_record(&mut conn, 6, &[]).await;
    write_record(&mut conn, 3 /* END_REQUEST */, &[0; 8]).await;
    let _ = conn.shutdown().await;
}

async fn write_record(conn: &mut TcpStream, record_type: u8, content: &[u8]) {
    let len = u16::try_from(content.len()).unwrap();
    let header: [u8; 8] = [
        1,
        record_type,
        0,
        1,
        (len >> 8) as u8,
        (len & 0xFF) as u8,
        0,
        0,
    ];
    conn.write_all(&header).await.unwrap();
    if !content.is_empty() {
        conn.write_all(content).await.unwrap();
    }
}

// ─── Test ───────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn https_handshake_routes_to_backend() {
    yerd_proxy::tls::init_crypto_once();

    let ca = CertAuthority::generate("Yerd Test CA", validity()).unwrap();
    let leaf = ca.issue_leaf(&["app.test".to_owned()], validity()).unwrap();
    let certified = Arc::new(parse_certified(leaf.cert_pem(), leaf.key_pem()));

    let fcgi_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fcgi_addr = fcgi_listener.local_addr().unwrap();
    let fake_task = tokio::spawn(run_fake_fcgi(
        fcgi_listener,
        b"Status: 200 OK\r\nContent-Type: text/plain\r\n\r\nsecure-hello".to_vec(),
    ));

    let proxy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_addr = proxy_listener.local_addr().unwrap();
    let http_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();

    let tld = Tld::new("test").unwrap();
    let cfg = RouterConfig::with_tld(tld);
    let mut router = SiteRouter::new(cfg);
    let site = Site::linked("app", PathBuf::from("/srv/www/app"), PhpVersion::new(8, 3)).unwrap();
    router.insert(site).unwrap();
    let router = Arc::new(tokio::sync::RwLock::new(router));

    let resolver = Arc::new(StaticResolver {
        backend: Backend::PhpFpmTcp { addr: fcgi_addr },
    });
    let cert_store = Arc::new(OneCertStore {
        host: "app.test".to_owned(),
        key: certified,
    });

    let https = HttpsBinding {
        listener: proxy_listener,
        public_port: Arc::new(std::sync::atomic::AtomicU16::new(proxy_addr.port())),
        cert_store,
    };

    let (tx_shutdown, rx_shutdown) = oneshot::channel::<()>();
    let proxy_task = tokio::spawn(async move {
        let _ = ProxyServer::serve(
            http_listener,
            Some(https),
            router,
            resolver,
            Arc::new(NoLoginTokens),
            None,
            async move {
                let _ = rx_shutdown.await;
            },
        )
        .await;
    });

    let response = client_https_get(proxy_addr, "app.test", ca.cert_pem(), "/").await;
    assert_eq!(response, b"secure-hello");

    let _ = tx_shutdown.send(());
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), proxy_task).await;
    let _ = fake_task.await;
}

/// The `Location` header on an HTTP→HTTPS redirect must track live updates to
/// `HttpsBinding::public_port` - this is what lets the daemon flip a secure
/// site's redirect target from the rootless fallback port to the well-known
/// port (and back) as a privileged-port redirect (e.g. macOS `yerd elevate
/// ports`) goes up or down, without restarting the proxy.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_redirect_tracks_live_public_port_updates() {
    yerd_proxy::tls::init_crypto_once();

    let ca = CertAuthority::generate("Yerd Test CA", validity()).unwrap();
    let leaf = ca.issue_leaf(&["app.test".to_owned()], validity()).unwrap();
    let certified = Arc::new(parse_certified(leaf.cert_pem(), leaf.key_pem()));

    let proxy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http_addr = http_listener.local_addr().unwrap();

    let tld = Tld::new("test").unwrap();
    let cfg = RouterConfig::with_tld(tld);
    let mut router = SiteRouter::new(cfg);
    let mut site =
        Site::linked("app", PathBuf::from("/srv/www/app"), PhpVersion::new(8, 3)).unwrap();
    site.set_secure(true);
    router.insert(site).unwrap();
    let router = Arc::new(tokio::sync::RwLock::new(router));

    let resolver = Arc::new(StaticResolver {
        backend: Backend::PhpFpmTcp {
            addr: "127.0.0.1:1".parse().unwrap(),
        },
    });
    let cert_store = Arc::new(OneCertStore {
        host: "app.test".to_owned(),
        key: certified,
    });

    let public_port = Arc::new(std::sync::atomic::AtomicU16::new(8443));
    let https = HttpsBinding {
        listener: proxy_listener,
        public_port: public_port.clone(),
        cert_store,
    };

    let (tx_shutdown, rx_shutdown) = oneshot::channel::<()>();
    let proxy_task = tokio::spawn(async move {
        let _ = ProxyServer::serve(
            http_listener,
            Some(https),
            router,
            resolver,
            Arc::new(NoLoginTokens),
            None,
            async move {
                let _ = rx_shutdown.await;
            },
        )
        .await;
    });

    let loc = redirect_location(http_addr, "app.test", "/dash").await;
    assert_eq!(loc.as_deref(), Some("https://app.test:8443/dash"));

    public_port.store(443, std::sync::atomic::Ordering::Relaxed);

    let loc = redirect_location(http_addr, "app.test", "/dash").await;
    assert_eq!(loc.as_deref(), Some("https://app.test/dash"));

    let _ = tx_shutdown.send(());
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), proxy_task).await;
}

async fn redirect_location(addr: SocketAddr, host: &str, path: &str) -> Option<String> {
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
    assert_eq!(resp.status(), hyper::StatusCode::MOVED_PERMANENTLY);
    resp.headers()
        .get(hyper::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
}

async fn client_https_get(addr: SocketAddr, sni_host: &str, ca_pem: &str, path: &str) -> Vec<u8> {
    let mut root_store = RootCertStore::empty();
    for cert in CertificateDer::pem_slice_iter(ca_pem.as_bytes()) {
        root_store.add(cert.unwrap().into_owned()).unwrap();
    }
    let config = ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
    let server_name = ServerName::try_from(sni_host.to_owned()).unwrap();

    let tcp = TcpStream::connect(addr).await.unwrap();
    let tls = connector.connect(server_name, tcp).await.unwrap();
    let io = TokioIo::new(tls);
    let (mut sender, conn) = hyper::client::conn::http1::handshake::<_, Empty<Bytes>>(io)
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let req = Request::builder()
        .method("GET")
        .uri(path)
        .header("Host", sni_host)
        .body(Empty::<Bytes>::new())
        .unwrap();
    let resp = sender.send_request(req).await.unwrap();
    resp.into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec()
}
