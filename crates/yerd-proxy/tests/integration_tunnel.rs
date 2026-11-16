//! Tunnel origin listener integration test: drive `ProxyServer::serve_tunnel`
//! against a fake FastCGI listener and a hyper client. Asserts public-host
//! routing, the share-password gate, and how tunnelled requests are presented
//! to PHP.

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
use tokio::sync::{oneshot, Mutex};

use yerd_core::{PhpVersion, RouterConfig, Site, SiteRouter, Tld};
use yerd_proxy::{
    Backend, BackendResolver, ProxyClientTls, ProxyError, ProxyServer, SharedTunnelRoutes,
    TunnelBinding, TunnelRoute,
};

const PUBLIC_HOST: &str = "app.example.com";
const PREPEND: &str = "/data/wordpress-public-url-prepend.php";

type Params = Arc<Mutex<HashMap<String, String>>>;

fn test_client_tls() -> Arc<ProxyClientTls> {
    let local = ProxyClientTls::no_verify_config().unwrap();
    let public = ProxyClientTls::no_verify_config().unwrap();
    Arc::new(ProxyClientTls::new(local, public))
}

struct TestResolver {
    backend: Backend,
    wordpress: bool,
}

#[async_trait]
impl BackendResolver for TestResolver {
    async fn backend_for(&self, _site: &Site) -> Result<Backend, ProxyError> {
        Ok(self.backend.clone())
    }

    async fn allows_direct_script_execution(&self, _site: &Site) -> bool {
        self.wordpress
    }

    async fn is_wordpress(&self, _site: &Site) -> bool {
        self.wordpress
    }
}

async fn run_fake_fcgi(listener: TcpListener, captured: Params) {
    let Ok((mut conn, _)) = listener.accept().await else {
        return;
    };
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
        if record_type == 4 {
            params_buf.extend_from_slice(&content);
        } else if record_type == 5 && content.is_empty() {
            break;
        }
    }
    *captured.lock().await = decode_params(&params_buf);
    let payload = b"Status: 200 OK\r\nContent-Type: text/plain\r\n\r\nfrom-php";
    write_record(&mut conn, 6, payload).await;
    write_record(&mut conn, 6, &[]).await;
    write_record(&mut conn, 3, &[0, 0, 0, 0, 0, 0, 0, 0]).await;
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

/// One running tunnel listener plus its fake FastCGI backend.
struct Harness {
    addr: SocketAddr,
    captured: Params,
    shutdown: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}

impl Harness {
    async fn stop(mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), &mut self.task).await;
    }
}

struct Setup {
    site: Site,
    routes: HashMap<String, TunnelRoute>,
    wordpress: bool,
    prepend: Option<PathBuf>,
}

impl Setup {
    fn new(site: Site) -> Self {
        let routes = HashMap::from([(
            PUBLIC_HOST.to_owned(),
            TunnelRoute {
                site: site.name().to_owned(),
                password: None,
            },
        )]);
        Self {
            site,
            routes,
            wordpress: false,
            prepend: Some(PathBuf::from(PREPEND)),
        }
    }

    async fn start(self) -> Harness {
        let fcgi_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let fcgi_addr = fcgi_listener.local_addr().unwrap();
        let captured: Params = Arc::new(Mutex::new(HashMap::new()));
        tokio::spawn(run_fake_fcgi(fcgi_listener, captured.clone()));

        let mut router = SiteRouter::new(RouterConfig::with_tld(Tld::new("test").unwrap()));
        router.insert(self.site).unwrap();
        let router = Arc::new(tokio::sync::RwLock::new(router));
        let route_map: SharedTunnelRoutes = Arc::new(tokio::sync::RwLock::new(self.routes));
        let resolver = Arc::new(TestResolver {
            backend: Backend::PhpFpmTcp { addr: fcgi_addr },
            wordpress: self.wordpress,
        });

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let binding = TunnelBinding {
            listener,
            routes: route_map,
            public_url_prepend: self.prepend,
        };
        let (tx, rx) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            let _ = ProxyServer::serve_tunnel(
                binding,
                router,
                resolver,
                Arc::new(AtomicBool::new(true)),
                test_client_tls(),
                async move {
                    let _ = rx.await;
                },
            )
            .await;
        });
        Harness {
            addr,
            captured,
            shutdown: Some(tx),
            task,
        }
    }
}

fn site(name: &str, root: PathBuf) -> Site {
    Site::linked(name, root, PhpVersion::new(8, 3)).unwrap()
}

struct Reply {
    status: u16,
    headers: hyper::HeaderMap,
    body: Vec<u8>,
}

async fn get(addr: SocketAddr, host: &str, path: &str, auth: Option<&str>) -> Reply {
    let stream = TcpStream::connect(addr).await.unwrap();
    let (mut sender, conn) =
        hyper::client::conn::http1::handshake::<_, Empty<Bytes>>(TokioIo::new(stream))
            .await
            .unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let mut builder = Request::builder()
        .method("GET")
        .uri(path)
        .header("Host", host);
    if let Some(value) = auth {
        builder = builder.header("Authorization", value);
    }
    let resp = sender
        .send_request(builder.body(Empty::<Bytes>::new()).unwrap())
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    let body = resp
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    Reply {
        status,
        headers,
        body,
    }
}

fn param<'a>(params: &'a HashMap<String, String>, name: &str) -> Option<&'a str> {
    params.get(name).map(String::as_str)
}

/// `user:secret` and `anyone:secret`, base64-encoded.
const AUTH_USER_SECRET: &str = "Basic dXNlcjpzZWNyZXQ=";
const AUTH_ANYONE_SECRET: &str = "Basic YW55b25lOnNlY3JldA==";
/// `user:wrong`, base64-encoded.
const AUTH_USER_WRONG: &str = "Basic dXNlcjp3cm9uZw==";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mapped_host_reaches_fpm_as_https_on_443_with_public_host() {
    let h = Setup::new(site("app", PathBuf::from("/srv/www/app")))
        .start()
        .await;

    let reply = get(h.addr, "App.Example.com", "/hello?x=1", None).await;

    assert_eq!(reply.status, 200);
    assert_eq!(reply.body, b"from-php");
    let params = h.captured.lock().await.clone();
    assert_eq!(param(&params, "HTTP_HOST"), Some("App.Example.com"));
    assert_eq!(param(&params, "SERVER_NAME"), Some("App.Example.com"));
    assert_eq!(param(&params, "HTTPS"), Some("on"));
    assert_eq!(param(&params, "SERVER_PORT"), Some("443"));
    assert_eq!(param(&params, "REQUEST_URI"), Some("/hello?x=1"));
    assert!(param(&params, "HTTP_AUTHORIZATION").is_none());
    assert!(param(&params, "PHP_VALUE").is_none());
    assert!(param(&params, "YERD_PUBLIC_URL").is_none());
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unmapped_host_is_404() {
    let h = Setup::new(site("app", PathBuf::from("/srv/www/app")))
        .start()
        .await;

    let reply = get(h.addr, "nope.example.com", "/", None).await;

    assert_eq!(reply.status, 404);
    assert_eq!(
        reply.headers.get("server").and_then(|v| v.to_str().ok()),
        Some(yerd_core::PROXY_SERVER_ID)
    );
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mapped_host_whose_site_is_missing_is_404() {
    let mut setup = Setup::new(site("app", PathBuf::from("/srv/www/app")));
    setup.routes.insert(
        "gone.example.com".to_owned(),
        TunnelRoute {
            site: "gone".to_owned(),
            password: None,
        },
    );
    let h = setup.start().await;

    let reply = get(h.addr, "gone.example.com", "/", None).await;

    assert_eq!(reply.status, 404);
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn password_gate_challenges_missing_and_wrong_credentials() {
    let mut setup = Setup::new(site("app", PathBuf::from("/srv/www/app")));
    setup
        .routes
        .get_mut(PUBLIC_HOST)
        .unwrap()
        .password
        .replace("secret".to_owned());
    let h = setup.start().await;

    for auth in [None, Some(AUTH_USER_WRONG), Some("Bearer secret")] {
        let reply = get(h.addr, PUBLIC_HOST, "/", auth).await;
        assert_eq!(reply.status, 401, "auth {auth:?}");
        assert_eq!(
            reply
                .headers
                .get("www-authenticate")
                .and_then(|v| v.to_str().ok()),
            Some(r#"Basic realm="Yerd share", charset="UTF-8""#)
        );
        assert_eq!(
            reply
                .headers
                .get("cache-control")
                .and_then(|v| v.to_str().ok()),
            Some("no-store")
        );
    }
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn right_password_with_any_username_reaches_fpm_without_authorization() {
    for auth in [AUTH_USER_SECRET, AUTH_ANYONE_SECRET] {
        let mut setup = Setup::new(site("app", PathBuf::from("/srv/www/app")));
        setup
            .routes
            .get_mut(PUBLIC_HOST)
            .unwrap()
            .password
            .replace("secret".to_owned());
        let h = setup.start().await;

        let reply = get(h.addr, PUBLIC_HOST, "/", Some(auth)).await;

        assert_eq!(reply.status, 200, "auth {auth}");
        let params = h.captured.lock().await.clone();
        assert!(param(&params, "HTTP_AUTHORIZATION").is_none());
        h.stop().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn secure_site_is_not_redirected() {
    let mut s = site("app", PathBuf::from("/srv/www/app"));
    s.set_secure(true);
    let h = Setup::new(s).start().await;

    let reply = get(h.addr, PUBLIC_HOST, "/", None).await;

    assert_eq!(reply.status, 200);
    assert!(reply.headers.get("location").is_none());
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wordpress_site_gets_the_public_url_prepend() {
    let mut setup = Setup::new(site("blog", PathBuf::from("/srv/www/blog")));
    setup.wordpress = true;
    let h = setup.start().await;

    let reply = get(h.addr, "app.example.com:443", "/", None).await;

    assert_eq!(reply.status, 200);
    let params = h.captured.lock().await.clone();
    assert_eq!(
        param(&params, "PHP_VALUE"),
        Some("auto_prepend_file=/data/wordpress-public-url-prepend.php")
    );
    assert_eq!(
        param(&params, "YERD_PUBLIC_URL"),
        Some("https://app.example.com")
    );
    assert!(param(&params, "YERD_LOGIN_USER").is_none());
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wordpress_site_without_a_prepend_script_gets_no_prepend() {
    let mut setup = Setup::new(site("blog", PathBuf::from("/srv/www/blog")));
    setup.wordpress = true;
    setup.prepend = None;
    let h = setup.start().await;

    let reply = get(h.addr, PUBLIC_HOST, "/", None).await;

    assert_eq!(reply.status, 200);
    let params = h.captured.lock().await.clone();
    assert!(param(&params, "PHP_VALUE").is_none());
    assert!(param(&params, "YERD_PUBLIC_URL").is_none());
    h.stop().await;
}

/// The tunnel path has no login-token consumer at all, so a one-click login
/// token on `/wp-admin` is neither consumed nor stripped: it reaches PHP as an
/// ordinary query param and no `YERD_LOGIN_USER` is sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn login_token_is_not_consumed_on_the_tunnel() {
    let mut setup = Setup::new(site("blog", PathBuf::from("/srv/www/blog")));
    setup.wordpress = true;
    let h = setup.start().await;

    let reply = get(h.addr, PUBLIC_HOST, "/wp-admin/?yerd_login_token=tok", None).await;

    assert_eq!(reply.status, 200);
    let params = h.captured.lock().await.clone();
    assert_eq!(param(&params, "QUERY_STRING"), Some("yerd_login_token=tok"));
    assert!(param(&params, "YERD_LOGIN_USER").is_none());
    assert_eq!(
        param(&params, "YERD_PUBLIC_URL"),
        Some("https://app.example.com")
    );
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn static_file_under_the_served_root_is_served() {
    let docroot = tempfile::tempdir().unwrap();
    std::fs::write(docroot.path().join("robots.txt"), b"User-agent: *\n").unwrap();
    let h = Setup::new(site("app", docroot.path().to_path_buf()))
        .start()
        .await;

    let reply = get(h.addr, PUBLIC_HOST, "/robots.txt", None).await;

    assert_eq!(reply.status, 200);
    assert_eq!(reply.body, b"User-agent: *\n");
    assert!(h.captured.lock().await.is_empty());
    h.stop().await;
}
