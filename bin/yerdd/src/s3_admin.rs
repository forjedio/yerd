//! S3 bucket administration for the managed versitygw gateway: list, create,
//! drop (with contents) and stats, plus reading the root credentials.
//!
//! This is the I/O edge. Every call goes over loopback HTTP to the running
//! gateway's real S3 API, signed with Signature Version 4. Request building,
//! signing, XML parsing and error classification are pure and unit-tested in
//! `yerd_services::s3`; the transport sits behind [`S3Http`] so the orchestration
//! here is tested against an in-memory fake. No credential, signature or secret
//! ever reaches an error message or a log line.

use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use yerd_core::network::ResolvedProxy;
use yerd_ipc::{BucketSummary, ErrorCode, Response};
use yerd_services::s3::{self, request, sigv4, xml, S3Failure, S3Request};
use yerd_services::{
    credentials, RootCredentials, ServiceRegistry, ServiceRunState, ServiceSnapshot, S3_REGION,
};
use yerd_supervise::Listen;

use crate::services::resolve_version;
use crate::state::DaemonState;

/// Keys requested per `ListObjectsV2` page (the S3 maximum).
const PAGE_SIZE: u32 = 1000;

/// `list buckets` - every bucket in the running gateway, sorted by name.
pub async fn list(state: &DaemonState) -> Response {
    let ctx = match prepare(state).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let http = match ReqwestS3Http::new() {
        Ok(h) => h,
        Err(r) => return r,
    };
    list_with(&ctx.admin(&http)).await
}

/// `create bucket <name>` - validated against the S3 naming rules first.
pub async fn create(name: &str, state: &DaemonState) -> Response {
    if let Err(r) = check_bucket_name(name) {
        return r;
    }
    let ctx = match prepare(state).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let http = match ReqwestS3Http::new() {
        Ok(h) => h,
        Err(r) => return r,
    };
    create_with(&ctx.admin(&http), name).await
}

/// `drop bucket <name>` - deletes every object, then the bucket.
pub async fn drop(name: &str, state: &DaemonState) -> Response {
    let ctx = match prepare(state).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let http = match ReqwestS3Http::new() {
        Ok(h) => h,
        Err(r) => return r,
    };
    drop_with(&ctx.admin(&http), name).await
}

/// `bucket stats <name>` - object count and total bytes.
pub async fn stats(name: &str, state: &DaemonState) -> Response {
    let ctx = match prepare(state).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let http = match ReqwestS3Http::new() {
        Ok(h) => h,
        Err(r) => return r,
    };
    stats_with(&ctx.admin(&http), name).await
}

/// `s3 credentials` - the endpoint and root access key, plus the secret only
/// when `show_secret` is set. Needs the gateway installed, not running: the
/// credentials are generated on first read if no start has created them yet.
/// The endpoint uses the port the gateway is bound to when it is running, else
/// the configured (or default) port.
pub async fn credentials(show_secret: bool, state: &DaemonState) -> Response {
    let Some(def) = ServiceRegistry::builtin().get(s3::SERVICE_ID) else {
        return unknown_service();
    };
    let (configured_version, configured_port) = {
        let cfg = state.config.lock().await;
        let inst = cfg.services.instances.get(def.id());
        (
            inst.and_then(|i| i.version.clone()),
            inst.and_then(|i| i.port),
        )
    };
    if let Err(r) = resolve_version(&def, configured_version.as_deref(), &state.dirs) {
        return r;
    }
    let creds = match credentials::load_or_create(&state.dirs, def.id()) {
        Ok(c) => c,
        Err(e) => return error(ErrorCode::Internal, e.to_string()),
    };
    let snapshots = {
        let mut mgr = state.service_manager.lock().await;
        mgr.snapshots()
    };
    let port = endpoint_port(
        live_port(&snapshots, def.id()),
        configured_port.unwrap_or(def.default_port()),
    );
    Response::S3Credentials {
        endpoint: endpoint(port),
        access_key: creds.access_key,
        secret_key: show_secret.then_some(creds.secret_key),
    }
}

/// The TCP port of `id`'s running instance, or `None` when it is not running
/// or does not listen on TCP.
fn live_port(snapshots: &[ServiceSnapshot], id: &str) -> Option<u16> {
    snapshots
        .iter()
        .find(|s| s.service == id && s.state == ServiceRunState::Running)
        .and_then(|s| match &s.listen {
            Some(Listen::TcpLoopback(addr)) => Some(addr.port()),
            _ => None,
        })
}

/// The port to advertise: the live one when running, else the configured one.
fn endpoint_port(live: Option<u16>, configured_or_default: u16) -> u16 {
    live.unwrap_or(configured_or_default)
}

fn endpoint(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

/// What every bucket operation needs, resolved once per request.
struct S3Ctx {
    port: u16,
    creds: RootCredentials,
}

impl S3Ctx {
    fn admin<'a, T: S3Http>(&'a self, http: &'a T) -> S3Admin<'a, T> {
        S3Admin {
            http,
            port: self.port,
            creds: &self.creds,
            now: unix_now,
            page_size: PAGE_SIZE,
        }
    }
}

/// Check versitygw is installed and running, and gather its live port and
/// credentials. The service-manager lock is released before any I/O.
async fn prepare(state: &DaemonState) -> Result<S3Ctx, Response> {
    let Some(def) = ServiceRegistry::builtin().get(s3::SERVICE_ID) else {
        return Err(unknown_service());
    };
    let configured = {
        let cfg = state.config.lock().await;
        cfg.services
            .instances
            .get(def.id())
            .and_then(|i| i.version.clone())
    };
    resolve_version(&def, configured.as_deref(), &state.dirs)?;
    let snapshots = {
        let mut mgr = state.service_manager.lock().await;
        mgr.snapshots()
    };
    let Some(port) = live_port(&snapshots, def.id()) else {
        return Err(error(
            ErrorCode::Internal,
            format!(
                "start {} first - managing buckets needs the running gateway: yerd service start {}",
                def.display_name(),
                def.id()
            ),
        ));
    };
    let creds = match credentials::load(&state.dirs, def.id()) {
        Ok(Some(c)) => c,
        Ok(None) => {
            return Err(error(
                ErrorCode::Internal,
                format!("credentials file missing; restart {}", def.id()),
            ))
        }
        Err(e) => return Err(error(ErrorCode::Internal, e.to_string())),
    };
    Ok(S3Ctx { port, creds })
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn error(code: ErrorCode, message: impl Into<String>) -> Response {
    Response::Error {
        code,
        message: message.into(),
    }
}

fn unknown_service() -> Response {
    error(
        ErrorCode::NotFound,
        format!("unknown service {:?}", s3::SERVICE_ID),
    )
}

/// A raw HTTP reply from the gateway.
struct HttpReply {
    status: u16,
    body: Vec<u8>,
}

/// The HTTP transport to the gateway, behind a trait so the orchestration is
/// testable without a server.
#[async_trait]
trait S3Http: Send + Sync {
    /// Send one request and return its status and body, or a transport error.
    async fn send(
        &self,
        method: &'static str,
        url: String,
        headers: Vec<(&'static str, String)>,
        body: Vec<u8>,
    ) -> Result<HttpReply, String>;
}

/// The real transport: a direct (never proxied) client from the daemon's
/// network factory, since the gateway is on loopback.
struct ReqwestS3Http {
    client: reqwest::Client,
}

impl ReqwestS3Http {
    fn new() -> Result<Self, Response> {
        crate::network::build_client(&ResolvedProxy::off())
            .map(|client| Self { client })
            .map_err(|e| error(ErrorCode::Internal, format!("build S3 client: {e}")))
    }
}

#[async_trait]
impl S3Http for ReqwestS3Http {
    async fn send(
        &self,
        method: &'static str,
        url: String,
        headers: Vec<(&'static str, String)>,
        body: Vec<u8>,
    ) -> Result<HttpReply, String> {
        let method = reqwest::Method::from_bytes(method.as_bytes()).map_err(|e| e.to_string())?;
        let mut req = self.client.request(method, url).body(body);
        for (name, value) in headers {
            req = req.header(name, value);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| format!("S3 gateway request failed: {e}"))?;
        let status = resp.status().as_u16();
        let body = resp
            .bytes()
            .await
            .map_err(|e| format!("S3 gateway reply failed: {e}"))?;
        Ok(HttpReply {
            status,
            body: body.to_vec(),
        })
    }
}

/// Why an S3 call failed.
#[derive(Debug)]
enum AdminError {
    /// The gateway answered with an S3 error.
    S3 { failure: S3Failure, message: String },
    /// The request could not be built, sent or understood.
    Transport(String),
}

/// Signs and sends S3 calls to the gateway on `127.0.0.1:<port>`.
struct S3Admin<'a, T: S3Http> {
    http: &'a T,
    port: u16,
    creds: &'a RootCredentials,
    now: fn() -> u64,
    page_size: u32,
}

impl<T: S3Http> S3Admin<'_, T> {
    async fn execute(&self, req: S3Request) -> Result<Vec<u8>, AdminError> {
        let host = format!("127.0.0.1:{}", self.port);
        let date = sigv4::AmzDate::from_unix((self.now)())
            .map_err(|e| AdminError::Transport(e.to_string()))?;
        let payload = sigv4::sha256_hex(&req.body);
        let signed = sigv4::sign(
            &sigv4::SigningInput {
                method: req.method,
                canonical_uri: &req.path,
                query: &req.query,
                host: &host,
                payload_sha256_hex: &payload,
            },
            self.creds,
            S3_REGION,
            &date,
        );
        let url = format!("http://{host}{}", req.url_path_and_query());
        let headers = vec![
            ("authorization", signed.authorization),
            ("x-amz-date", signed.x_amz_date),
            ("x-amz-content-sha256", signed.x_amz_content_sha256),
        ];
        let reply = self
            .http
            .send(req.method, url, headers, req.body)
            .await
            .map_err(AdminError::Transport)?;
        if (200..300).contains(&reply.status) {
            return Ok(reply.body);
        }
        let text = String::from_utf8_lossy(&reply.body);
        Err(match xml::parse_error(&text) {
            Ok(body) => AdminError::S3 {
                failure: s3::classify(&body.code),
                message: if body.message.is_empty() {
                    body.code
                } else {
                    body.message
                },
            },
            Err(_) => AdminError::Transport(format!(
                "S3 gateway answered HTTP {} without an S3 error body",
                reply.status
            )),
        })
    }

    async fn list_buckets(&self) -> Result<Vec<String>, AdminError> {
        let body = self.execute(request::list_buckets()).await?;
        let mut names = xml::parse_list_buckets(&String::from_utf8_lossy(&body))
            .map_err(|e| AdminError::Transport(e.to_string()))?;
        names.sort();
        Ok(names)
    }

    async fn create_bucket(&self, name: &str) -> Result<(), AdminError> {
        self.execute(request::create_bucket(name)).await.map(|_| ())
    }

    async fn list_page(
        &self,
        name: &str,
        continuation: Option<&str>,
    ) -> Result<xml::ObjectsPage, AdminError> {
        let body = self
            .execute(request::list_objects_v2(name, continuation, self.page_size))
            .await?;
        xml::parse_list_objects_v2(&String::from_utf8_lossy(&body))
            .map_err(|e| AdminError::Transport(e.to_string()))
    }

    async fn bucket_stats(&self, name: &str) -> Result<(u64, u64), AdminError> {
        let mut objects: u64 = 0;
        let mut bytes: u64 = 0;
        let mut token: Option<String> = None;
        loop {
            let page = self.list_page(name, token.as_deref()).await?;
            for (_, size) in &page.objects {
                objects = objects.saturating_add(1);
                bytes = bytes.saturating_add(*size);
            }
            match page.next_token {
                Some(next) => token = Some(next),
                None => return Ok((objects, bytes)),
            }
        }
    }

    /// Delete every object page by page (always relisting from the start), then
    /// the bucket. A page identical to the previous one means the deletes are not
    /// taking effect, which is reported instead of looping forever.
    async fn drop_bucket(&self, name: &str) -> Result<u64, AdminError> {
        let mut removed: u64 = 0;
        let mut previous: Vec<(String, u64)> = Vec::new();
        loop {
            let page = self.list_page(name, None).await?;
            if page.objects.is_empty() {
                break;
            }
            if page.objects == previous {
                return Err(AdminError::Transport(format!(
                    "objects in bucket {name} could not be deleted"
                )));
            }
            for (key, _) in &page.objects {
                self.execute(request::delete_object(name, key)).await?;
                removed = removed.saturating_add(1);
            }
            previous = page.objects;
        }
        self.execute(request::delete_bucket(name)).await?;
        Ok(removed)
    }
}

fn admin_error(e: AdminError, bucket: Option<&str>) -> Response {
    match e {
        AdminError::S3 {
            failure: S3Failure::AlreadyExists,
            message,
        } => match bucket {
            Some(name) => error(
                ErrorCode::AlreadyExists,
                format!("a bucket called {name} already exists"),
            ),
            None => error(ErrorCode::AlreadyExists, message),
        },
        AdminError::S3 {
            failure: S3Failure::NoSuchBucket,
            message,
        } => match bucket {
            Some(name) => error(ErrorCode::NotFound, format!("no bucket called {name}")),
            None => error(ErrorCode::NotFound, message),
        },
        AdminError::S3 {
            failure: S3Failure::BucketNotEmpty | S3Failure::Other,
            message,
        }
        | AdminError::Transport(message) => error(ErrorCode::Internal, message),
    }
}

async fn list_with<T: S3Http>(admin: &S3Admin<'_, T>) -> Response {
    match admin.list_buckets().await {
        Ok(names) => Response::Buckets {
            buckets: names
                .into_iter()
                .map(|name| BucketSummary { name })
                .collect(),
        },
        Err(e) => admin_error(e, None),
    }
}

fn check_bucket_name(name: &str) -> Result<(), Response> {
    s3::validate_bucket_name(name).map_err(|e| error(ErrorCode::InvalidPath, e.to_string()))
}

async fn create_with<T: S3Http>(admin: &S3Admin<'_, T>, name: &str) -> Response {
    if let Err(r) = check_bucket_name(name) {
        return r;
    }
    match admin.create_bucket(name).await {
        Ok(()) => Response::Ok,
        Err(e) => admin_error(e, Some(name)),
    }
}

async fn drop_with<T: S3Http>(admin: &S3Admin<'_, T>, name: &str) -> Response {
    match admin.drop_bucket(name).await {
        Ok(objects_removed) => Response::BucketDropped { objects_removed },
        Err(e) => admin_error(e, Some(name)),
    }
}

async fn stats_with<T: S3Http>(admin: &S3Admin<'_, T>, name: &str) -> Response {
    match admin.bucket_stats(name).await {
        Ok((objects, bytes)) => Response::BucketStats { objects, bytes },
        Err(e) => admin_error(e, Some(name)),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::collections::BTreeMap;
    use std::net::SocketAddr;
    use std::sync::Mutex;

    use yerd_services::{version, ServiceVersion};

    use super::*;
    use crate::test_support::state_in;

    #[derive(Debug, Clone)]
    struct Recorded {
        method: &'static str,
        url: String,
        header_names: Vec<&'static str>,
    }

    #[derive(Default)]
    struct FakeS3 {
        buckets: Mutex<BTreeMap<String, BTreeMap<String, u64>>>,
        requests: Mutex<Vec<Recorded>>,
        refuse_delete_bucket: bool,
    }

    impl FakeS3 {
        fn with_bucket(self, name: &str, objects: &[(&str, u64)]) -> Self {
            self.buckets.lock().unwrap().insert(
                name.to_owned(),
                objects.iter().map(|(k, s)| ((*k).to_owned(), *s)).collect(),
            );
            self
        }

        fn requests(&self) -> Vec<Recorded> {
            self.requests.lock().unwrap().clone()
        }
    }

    fn percent_decode(s: &str) -> String {
        let mut out = Vec::new();
        let mut rest = s.as_bytes();
        while let Some((&b, tail)) = rest.split_first() {
            if b == b'%' {
                let hex = std::str::from_utf8(tail.get(..2).unwrap()).unwrap();
                out.push(u8::from_str_radix(hex, 16).unwrap());
                rest = tail.get(2..).unwrap();
            } else {
                out.push(b);
                rest = tail;
            }
        }
        String::from_utf8(out).unwrap()
    }

    fn xml_escape(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }

    fn s3_error(status: u16, code: &str, message: &str) -> HttpReply {
        HttpReply {
            status,
            body: format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Error><Code>{code}</Code><Message>{message}</Message></Error>"
            )
            .into_bytes(),
        }
    }

    fn ok(body: String) -> HttpReply {
        HttpReply {
            status: 200,
            body: body.into_bytes(),
        }
    }

    #[async_trait]
    #[allow(clippy::too_many_lines)]
    impl S3Http for FakeS3 {
        async fn send(
            &self,
            method: &'static str,
            url: String,
            headers: Vec<(&'static str, String)>,
            _body: Vec<u8>,
        ) -> Result<HttpReply, String> {
            self.requests.lock().unwrap().push(Recorded {
                method,
                url: url.clone(),
                header_names: headers.iter().map(|(n, _)| *n).collect(),
            });
            let target = url
                .strip_prefix("http://127.0.0.1:7070")
                .expect("requests go to the configured loopback port");
            let (path, query) = target.split_once('?').unwrap_or((target, ""));
            let params: BTreeMap<String, String> = query
                .split('&')
                .filter(|p| !p.is_empty())
                .map(|p| {
                    let (k, v) = p.split_once('=').unwrap();
                    (percent_decode(k), percent_decode(v))
                })
                .collect();
            let rest = path.strip_prefix('/').unwrap();
            let (bucket, key) = match rest.split_once('/') {
                Some((b, k)) => (percent_decode(b), Some(percent_decode(k))),
                None => (percent_decode(rest), None),
            };
            let mut buckets = self.buckets.lock().unwrap();
            if bucket.is_empty() {
                let names: String = buckets
                    .keys()
                    .map(|n| ["<Bucket><Name>", &xml_escape(n), "</Name></Bucket>"].concat())
                    .collect();
                return Ok(ok(format!(
                    "<ListAllMyBucketsResult><Buckets>{names}</Buckets></ListAllMyBucketsResult>"
                )));
            }
            match (method, key) {
                ("PUT", None) => {
                    if buckets.contains_key(&bucket) {
                        return Ok(s3_error(409, "BucketAlreadyOwnedByYou", "owned"));
                    }
                    buckets.insert(bucket, BTreeMap::new());
                    Ok(ok(String::new()))
                }
                ("DELETE", None) => match buckets.get(&bucket) {
                    None => Ok(s3_error(404, "NoSuchBucket", "missing")),
                    Some(objs) if !objs.is_empty() || self.refuse_delete_bucket => Ok(s3_error(
                        409,
                        "BucketNotEmpty",
                        "The bucket you tried to delete is not empty.",
                    )),
                    Some(_) => {
                        buckets.remove(&bucket);
                        Ok(HttpReply {
                            status: 204,
                            body: Vec::new(),
                        })
                    }
                },
                ("DELETE", Some(key)) => {
                    if let Some(objs) = buckets.get_mut(&bucket) {
                        objs.remove(&key);
                    }
                    Ok(HttpReply {
                        status: 204,
                        body: Vec::new(),
                    })
                }
                ("GET", None) => {
                    let Some(objs) = buckets.get(&bucket) else {
                        return Ok(s3_error(404, "NoSuchBucket", "missing"));
                    };
                    assert_eq!(params.get("list-type").map(String::as_str), Some("2"));
                    let max: usize = params.get("max-keys").unwrap().parse().unwrap();
                    let after = params.get("continuation-token");
                    let remaining: Vec<(&String, &u64)> = objs
                        .iter()
                        .filter(|(k, _)| after.map_or(true, |a| *k > a))
                        .collect();
                    let page: Vec<_> = remaining.iter().take(max).collect();
                    let truncated = remaining.len() > max;
                    let contents: String = page
                        .iter()
                        .map(|(k, s)| {
                            [
                                "<Contents><Key>",
                                &xml_escape(k),
                                "</Key><Size>",
                                &s.to_string(),
                                "</Size></Contents>",
                            ]
                            .concat()
                        })
                        .collect();
                    let next = if truncated {
                        format!(
                            "<NextContinuationToken>{}</NextContinuationToken>",
                            xml_escape(page.last().unwrap().0)
                        )
                    } else {
                        String::new()
                    };
                    Ok(ok(format!(
                        "<ListBucketResult>{next}<IsTruncated>{truncated}</IsTruncated>{contents}</ListBucketResult>"
                    )))
                }
                other => panic!("unexpected request {other:?}"),
            }
        }
    }

    fn creds() -> RootCredentials {
        RootCredentials {
            access_key: "yerd_test000001".to_owned(),
            secret_key: "s".repeat(40),
        }
    }

    fn fixed_now() -> u64 {
        1_369_353_600
    }

    fn admin<'a>(
        fake: &'a FakeS3,
        creds: &'a RootCredentials,
        page_size: u32,
    ) -> S3Admin<'a, FakeS3> {
        S3Admin {
            http: fake,
            port: 7070,
            creds,
            now: fixed_now,
            page_size,
        }
    }

    fn five_objects() -> Vec<(&'static str, u64)> {
        vec![("a", 1), ("b", 2), ("c d/ü", 3), ("e&f", 4), ("g", 5)]
    }

    fn err_parts(r: Response) -> (ErrorCode, String) {
        match r {
            Response::Error { code, message } => (code, message),
            other => panic!("expected Response::Error, got {other:?}"),
        }
    }

    fn assert_all_signed(fake: &FakeS3) {
        for r in fake.requests() {
            assert_eq!(
                r.header_names,
                vec!["authorization", "x-amz-date", "x-amz-content-sha256"],
                "{r:?}"
            );
        }
    }

    #[tokio::test]
    async fn stats_pages_through_continuation_tokens() {
        let fake = FakeS3::default().with_bucket("media", &five_objects());
        let c = creds();
        let r = stats_with(&admin(&fake, &c, 2), "media").await;
        assert_eq!(
            r,
            Response::BucketStats {
                objects: 5,
                bytes: 15
            }
        );
        let urls: Vec<String> = fake.requests().into_iter().map(|r| r.url).collect();
        assert_eq!(
            urls,
            vec![
                "http://127.0.0.1:7070/media?list-type=2&max-keys=2".to_owned(),
                "http://127.0.0.1:7070/media?continuation-token=b&list-type=2&max-keys=2"
                    .to_owned(),
                "http://127.0.0.1:7070/media?continuation-token=e%26f&list-type=2&max-keys=2"
                    .to_owned(),
            ]
        );
        assert_all_signed(&fake);
    }

    #[tokio::test]
    async fn drop_deletes_every_object_then_the_bucket() {
        let fake = FakeS3::default().with_bucket("media", &five_objects());
        let c = creds();
        let r = drop_with(&admin(&fake, &c, 2), "media").await;
        assert_eq!(r, Response::BucketDropped { objects_removed: 5 });
        assert!(fake.buckets.lock().unwrap().is_empty());
        let reqs = fake.requests();
        let last = reqs.last().unwrap();
        assert_eq!(
            (last.method, last.url.as_str()),
            ("DELETE", "http://127.0.0.1:7070/media")
        );
        let object_deletes = reqs
            .iter()
            .filter(|r| r.method == "DELETE" && r.url.starts_with("http://127.0.0.1:7070/media/"))
            .count();
        assert_eq!(object_deletes, 5);
        assert!(reqs
            .iter()
            .any(|r| r.url == "http://127.0.0.1:7070/media/c%20d/%C3%BC"));
        assert_all_signed(&fake);
    }

    #[tokio::test]
    async fn drop_surfaces_bucket_not_empty_message() {
        let fake = FakeS3 {
            refuse_delete_bucket: true,
            ..FakeS3::default()
        }
        .with_bucket("media", &[("a", 1)]);
        let c = creds();
        let (code, msg) = err_parts(drop_with(&admin(&fake, &c, 2), "media").await);
        assert_eq!(code, ErrorCode::Internal);
        assert_eq!(msg, "The bucket you tried to delete is not empty.");
    }

    #[tokio::test]
    async fn create_then_duplicate_is_already_exists() {
        let fake = FakeS3::default();
        let c = creds();
        let a = admin(&fake, &c, 2);
        assert_eq!(create_with(&a, "assets").await, Response::Ok);
        let (code, msg) = err_parts(create_with(&a, "assets").await);
        assert_eq!(code, ErrorCode::AlreadyExists);
        assert_eq!(msg, "a bucket called assets already exists");
        assert_all_signed(&fake);
    }

    #[tokio::test]
    async fn missing_bucket_is_not_found() {
        let fake = FakeS3::default();
        let c = creds();
        let a = admin(&fake, &c, 2);
        for r in [stats_with(&a, "nope").await, drop_with(&a, "nope").await] {
            let (code, msg) = err_parts(r);
            assert_eq!(code, ErrorCode::NotFound);
            assert_eq!(msg, "no bucket called nope");
        }
    }

    #[tokio::test]
    async fn invalid_name_never_reaches_the_gateway() {
        let fake = FakeS3::default();
        let c = creds();
        let (code, msg) = err_parts(create_with(&admin(&fake, &c, 2), "Bad_Name").await);
        assert_eq!(code, ErrorCode::InvalidPath);
        assert!(msg.contains("lowercase"), "{msg}");
        assert!(fake.requests().is_empty());
    }

    #[tokio::test]
    async fn list_is_sorted_by_name() {
        let fake = FakeS3::default()
            .with_bucket("media", &[])
            .with_bucket("assets", &[]);
        let c = creds();
        assert_eq!(
            list_with(&admin(&fake, &c, 2)).await,
            Response::Buckets {
                buckets: vec![
                    BucketSummary {
                        name: "assets".into()
                    },
                    BucketSummary {
                        name: "media".into()
                    },
                ]
            }
        );
    }

    fn snapshot(service: &str, state: ServiceRunState, listen: Option<Listen>) -> ServiceSnapshot {
        ServiceSnapshot {
            service: service.to_owned(),
            version: None,
            state,
            pid: None,
            listen,
        }
    }

    fn tcp(port: u16) -> Listen {
        Listen::TcpLoopback(SocketAddr::from(([127, 0, 0, 1], port)))
    }

    #[test]
    fn live_port_table() {
        let cases = [
            (
                vec![snapshot(
                    "versitygw",
                    ServiceRunState::Running,
                    Some(tcp(7171)),
                )],
                Some(7171),
            ),
            (
                vec![snapshot(
                    "versitygw",
                    ServiceRunState::Failed,
                    Some(tcp(7171)),
                )],
                None,
            ),
            (vec![], None),
            (
                vec![snapshot("redis", ServiceRunState::Running, Some(tcp(6379)))],
                None,
            ),
            (
                vec![snapshot(
                    "versitygw",
                    ServiceRunState::Running,
                    Some(Listen::UnixSocket("/tmp/x.sock".into())),
                )],
                None,
            ),
        ];
        for (snaps, want) in cases {
            assert_eq!(live_port(&snaps, "versitygw"), want, "{snaps:?}");
        }
    }

    #[test]
    fn endpoint_port_prefers_live() {
        assert_eq!(endpoint_port(Some(7171), 7070), 7171);
        assert_eq!(endpoint_port(None, 7070), 7070);
    }

    fn stage_install(state: &DaemonState) {
        let v: ServiceVersion = "1.7.0".parse().unwrap();
        let bin = version::install_dir(&state.dirs, "versitygw", &v).join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("versitygw"), b"#!fake").unwrap();
    }

    #[tokio::test]
    async fn not_installed_is_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        for r in [
            list(&state).await,
            stats("media", &state).await,
            drop("media", &state).await,
            create("media", &state).await,
            credentials(false, &state).await,
        ] {
            let (code, msg) = err_parts(r);
            assert_eq!(code, ErrorCode::NotFound);
            assert!(msg.contains("yerd service install versitygw"), "{msg}");
        }
    }

    #[tokio::test]
    async fn installed_but_stopped_asks_to_start() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        stage_install(&state);
        let (code, msg) = err_parts(list(&state).await);
        assert_eq!(code, ErrorCode::Internal);
        assert!(msg.starts_with("start S3 (versitygw) first"), "{msg}");
    }

    #[tokio::test]
    async fn credentials_while_stopped_use_config_port_and_hide_secret() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        stage_install(&state);
        state.config.lock().await.services.instances.insert(
            "versitygw".to_owned(),
            yerd_config::ServiceInstance {
                port: Some(17070),
                ..yerd_config::ServiceInstance::default()
            },
        );
        let hidden = credentials(false, &state).await;
        let Response::S3Credentials {
            endpoint,
            access_key,
            secret_key,
        } = hidden
        else {
            panic!("expected s3_credentials, got {hidden:?}");
        };
        assert_eq!(endpoint, "http://127.0.0.1:17070");
        assert!(access_key.starts_with("yerd_"));
        assert_eq!(secret_key, None);
        let stored = credentials::load(&state.dirs, "versitygw")
            .unwrap()
            .unwrap();
        assert_eq!(access_key, stored.access_key);
        let shown = credentials(true, &state).await;
        let Response::S3Credentials { secret_key, .. } = shown else {
            panic!("expected s3_credentials, got {shown:?}");
        };
        assert_eq!(secret_key, Some(stored.secret_key));
    }

    #[tokio::test]
    async fn credentials_default_to_the_default_port() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        stage_install(&state);
        let r = credentials(false, &state).await;
        let Response::S3Credentials { endpoint, .. } = r else {
            panic!("expected s3_credentials, got {r:?}");
        };
        assert_eq!(endpoint, "http://127.0.0.1:7070");
    }
}
