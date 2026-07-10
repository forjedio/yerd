//! Reverse-proxy integration test: whole-host proxy, path-into-site rule with
//! PHP fall-through, and 502 on a dead upstream. Drives `ProxyServer::serve`
//! against a fake HTTP upstream that echoes the request path and whether an
//! `X-Forwarded-For` header was present (proxied requests carry it; the plain
//! FrankenPHP fall-through does not).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::doc_markdown
)]

use std::net::SocketAddr;
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

use yerd_core::{
    PhpVersion, ProxyRule, ProxySite, RouterConfig, Site, SiteRouter, Tld, UpstreamTarget,
};
use yerd_proxy::{Backend, BackendResolver, CertStore, ProxyClientTls, ProxyError, ProxyServer};

struct StaticResolver {
    backend: Backend,
}

#[async_trait]
impl BackendResolver for StaticResolver {
    async fn backend_for(&self, _site: &Site) -> Result<Backend, ProxyError> {
        Ok(self.backend.clone())
    }
}

struct StubCertStore;
impl CertStore for StubCertStore {
    fn certified_key(&self, _: &str) -> Option<Arc<rustls::sign::CertifiedKey>> {
        None
    }
}
impl std::fmt::Debug for StubCertStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StubCertStore")
    }
}

struct NoLoginTokens;
impl yerd_proxy::LoginTokenConsumer for NoLoginTokens {
    fn consume(&self, _site: &str, _token: &str) -> Option<String> {
        None
    }
}

fn test_client_tls() -> Arc<ProxyClientTls> {
    let local = ProxyClientTls::no_verify_config().unwrap();
    let public = ProxyClientTls::no_verify_config().unwrap();
    Arc::new(ProxyClientTls::new(local, public))
}

/// Spawn a fake HTTP/1.1 upstream: for each connection, read the request head
/// and reply with a body echoing the request path and whether `x-forwarded-for`
/// was present. Runs until `shutdown` fires.
async fn spawn_upstream(shutdown: oneshot::Receiver<()>) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let mut shutdown = shutdown;
        loop {
            tokio::select! {
                _ = &mut shutdown => break,
                accepted = listener.accept() => {
                    let Ok((mut stream, _)) = accepted else { continue };
                    tokio::spawn(async move {
                        let mut buf = Vec::new();
                        let mut chunk = [0u8; 1024];
                        let head_end = loop {
                            let n = match stream.read(&mut chunk).await {
                                Ok(0) | Err(_) => return,
                                Ok(n) => n,
                            };
                            buf.extend_from_slice(&chunk[..n]);
                            if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                                break pos + 4;
                            }
                        };
                        let text = String::from_utf8_lossy(&buf[..head_end]).to_ascii_lowercase();
                        let path = String::from_utf8_lossy(&buf[..head_end])
                            .lines()
                            .next()
                            .and_then(|line| line.split_whitespace().nth(1))
                            .unwrap_or("?")
                            .to_owned();
                        let xff = text.contains("x-forwarded-for:");
                        let conn_upgrade = text
                            .lines()
                            .filter(|l| l.starts_with("connection:"))
                            .any(|l| l.contains("upgrade"));
                        let want: usize = text
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .and_then(|v| v.trim().parse().ok())
                            .unwrap_or(0);
                        let mut body_bytes = buf.len().saturating_sub(head_end);
                        while body_bytes < want {
                            let n = match stream.read(&mut chunk).await {
                                Ok(0) | Err(_) => break,
                                Ok(n) => n,
                            };
                            body_bytes += n;
                        }
                        let body = format!(
                            "path={path};xff={};bodylen={body_bytes};connupg={}",
                            u8::from(xff),
                            u8::from(conn_upgrade)
                        );
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = stream.write_all(resp.as_bytes()).await;
                        let _ = stream.flush().await;
                    });
                }
            }
        }
    });
    addr
}

async fn client_get(addr: SocketAddr, host: &str, path: &str) -> (u16, String) {
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
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&body).into_owned())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn whole_host_and_path_rules_and_bad_gateway() {
    yerd_proxy::tls::init_crypto_once();

    let (up_tx, up_rx) = oneshot::channel::<()>();
    let upstream = spawn_upstream(up_rx).await;
    let upstream_url = format!("http://127.0.0.1:{}", upstream.port());

    // A controlled failing upstream: accept each connection and immediately close
    // it, so a proxied request deterministically fails with 502 - no reliance on
    // an ephemeral port staying free (which would race the request's connect).
    let dead_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dead_port = dead_listener.local_addr().unwrap().port();
    let (dead_tx, mut dead_rx) = oneshot::channel::<()>();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut dead_rx => break,
                accepted = dead_listener.accept() => {
                    if let Ok((stream, _)) = accepted {
                        drop(stream);
                    }
                }
            }
        }
    });

    let proxy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_addr = proxy_listener.local_addr().unwrap();

    let cfg = RouterConfig::with_tld(Tld::new("test").unwrap());
    let mut router = SiteRouter::new(cfg);
    let app = Site::linked("app", "/srv/www/app", PhpVersion::new(8, 3)).unwrap();
    router.insert(app).unwrap();
    router.set_proxy_rules(
        "app",
        vec![ProxyRule::new("/ws", UpstreamTarget::from_url_str(&upstream_url).unwrap()).unwrap()],
    );
    router
        .insert_proxy(
            ProxySite::new(
                "reverb",
                UpstreamTarget::from_url_str(&upstream_url).unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
    router
        .insert_proxy(
            ProxySite::new(
                "dead",
                UpstreamTarget::from_url_str(&format!("http://127.0.0.1:{dead_port}")).unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
    let router = Arc::new(tokio::sync::RwLock::new(router));

    let resolver = Arc::new(StaticResolver {
        backend: Backend::FrankenPhp { addr: upstream },
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

    let (status, body) = client_get(proxy_addr, "reverb.test", "/foo").await;
    assert_eq!(status, 200);
    assert_eq!(body, "path=/foo;xff=1;bodylen=0;connupg=0");

    let (status, body) = client_get(proxy_addr, "app.test", "/ws/x").await;
    assert_eq!(status, 200);
    assert_eq!(body, "path=/ws/x;xff=1;bodylen=0;connupg=0");

    let (status, body) = client_get(proxy_addr, "app.test", "/plain").await;
    assert_eq!(status, 200);
    assert_eq!(body, "path=/plain;xff=0;bodylen=0;connupg=0");

    let (status, body) = client_post(proxy_addr, "reverb.test", "/submit", "hello-body").await;
    assert_eq!(status, 200);
    assert_eq!(body, "path=/submit;xff=1;bodylen=10;connupg=0");

    let (status, _) = client_get(proxy_addr, "dead.test", "/").await;
    assert_eq!(status, 502);

    let _ = tx_shutdown.send(());
    let _ = up_tx.send(());
    let _ = dead_tx.send(());
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), proxy_task).await;
}

async fn client_post(addr: SocketAddr, host: &str, path: &str, body: &str) -> (u16, String) {
    use http_body_util::Full;
    let stream = TcpStream::connect(addr).await.unwrap();
    let io = TokioIo::new(stream);
    let (mut sender, conn) = hyper::client::conn::http1::handshake::<_, Full<Bytes>>(io)
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let req = Request::builder()
        .method("POST")
        .uri(path)
        .header("Host", host)
        .header("Content-Length", body.len())
        .body(Full::new(Bytes::from(body.to_owned())))
        .unwrap();
    let resp = sender.send_request(req).await.unwrap();
    let status = resp.status().as_u16();
    let out = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&out).into_owned())
}
