//! LAN remote-device bootstrap endpoint.
//!
//! A small hyper service on `0.0.0.0:<lan_setup_port>`, spawned only while LAN
//! mode is on and a LAN IP was discovered. It implements the R2-C1
//! fingerprint-anchored trust flow:
//!
//! 1. The device fetches the **public CA** over plain HTTP (`GET
//!    /remote-setup/ca?code=…`) - the only plaintext step, because the device has
//!    no trust yet.
//! 2. It verifies the CA's DER SHA-256 against the fingerprint the operator
//!    copy-pasted from `yerd remote-setup` (out of band, never over the wire).
//! 3. It fetches the **installer script** over HTTPS (`GET /remote-setup?code=…`)
//!    validated against that just-verified CA. The script route is HTTPS-only.
//!
//! One TCP port serves both by peeking the first byte (TLS `0x16` handshake vs an
//! ASCII HTTP verb). The TLS side uses a fixed single-cert config whose leaf
//! carries the LAN IP as an iPAddress SAN (an IP-literal client sends no SNI).
//! The CA **private key never leaves the daemon**; only the public cert and the
//! daemon-held IP-SAN leaf are involved.

use std::convert::Infallible;
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Instant;

use http_body_util::Full;
use hyper::body::Bytes;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio_rustls::TlsAcceptor;

use crate::state::DaemonState;

/// Immutable context the endpoint serves from, built once at spawn.
pub struct SetupContext {
    /// The public CA PEM, read into memory once (never a live filesystem path,
    /// and never the private key).
    pub ca_pem: Vec<u8>,
    /// The served TLD (e.g. `"test"`), interpolated into the installer script.
    pub tld: String,
    /// The DNS responder port the device points at.
    pub dns_port: u16,
    /// The host's LAN IPv4 - the DNS target the device configures.
    pub server_ip: Ipv4Addr,
    /// Shared daemon state, for the one-time code store.
    pub state: Arc<DaemonState>,
}

/// A private-IP peer gets this many wrong-code tries before the code is
/// invalidated (lockout). The code is 128-bit, so brute-force is already
/// infeasible; this is defence in depth.
const MAX_CODE_ATTEMPTS: u32 = 20;

/// Serve until `shutdown` resolves. Non-LAN peers are dropped at accept.
pub async fn serve(
    listener: TcpListener,
    tls_config: Arc<rustls::ServerConfig>,
    ctx: Arc<SetupContext>,
    mut shutdown: watch::Receiver<bool>,
) {
    let acceptor = TlsAcceptor::from(tls_config);
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, peer) = match accepted {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::debug!(error = %e, "lan setup: accept failed");
                        continue;
                    }
                };
                // Blast-radius reduction (not auth): only private/link-local peers.
                if !yerd_core::is_lan_source(peer.ip()) {
                    continue;
                }
                let acceptor = acceptor.clone();
                let ctx = Arc::clone(&ctx);
                tokio::spawn(async move {
                    handle_conn(stream, acceptor, ctx).await;
                });
            }
            _ = shutdown.changed() => break,
        }
    }
}

async fn handle_conn(stream: TcpStream, acceptor: TlsAcceptor, ctx: Arc<SetupContext>) {
    let mut first = [0u8; 1];
    match stream.peek(&mut first).await {
        Ok(n) if n >= 1 => {}
        _ => return,
    }
    if first[0] == 0x16 {
        match acceptor.accept(stream).await {
            Ok(tls) => serve_conn(TokioIo::new(tls), ctx, true).await,
            Err(e) => tracing::debug!(error = %e, "lan setup: TLS handshake failed"),
        }
    } else {
        serve_conn(TokioIo::new(stream), ctx, false).await;
    }
}

async fn serve_conn<I>(io: I, ctx: Arc<SetupContext>, tls: bool)
where
    I: hyper::rt::Read + hyper::rt::Write + Unpin + 'static,
{
    let service = service_fn(move |req| {
        let ctx = Arc::clone(&ctx);
        async move { Ok::<_, Infallible>(handle_request(&req, &ctx, tls).await) }
    });
    if let Err(e) = hyper::server::conn::http1::Builder::new()
        .serve_connection(io, service)
        .await
    {
        tracing::debug!(error = %e, "lan setup: connection error");
    }
}

/// What the endpoint decided to reply with, independent of the hyper types so it
/// can be unit-tested against a seeded [`DaemonState`].
#[derive(Debug, PartialEq, Eq)]
enum Decision {
    /// Serve the public CA PEM.
    Ca,
    /// Serve the installer script.
    Script,
    /// A plain-text status reply (error / not-found).
    Text(StatusCode, &'static str),
}

async fn decide(
    ctx: &SetupContext,
    is_get: bool,
    path: &str,
    query: Option<&str>,
    tls: bool,
) -> Decision {
    if !is_get {
        return Decision::Text(StatusCode::METHOD_NOT_ALLOWED, "GET only");
    }
    let Some(code) = pure::extract_code(query) else {
        return Decision::Text(StatusCode::FORBIDDEN, "missing code");
    };
    match pure::classify(path) {
        pure::Route::Ca => {
            // Validate but do NOT consume - the CA is public and gets verified
            // out of band; consuming here would 403 the terminal script fetch.
            if check_code(&ctx.state, &code, false).await {
                Decision::Ca
            } else {
                Decision::Text(StatusCode::FORBIDDEN, "invalid code")
            }
        }
        pure::Route::Script => {
            if !tls {
                return Decision::Text(
                    StatusCode::FORBIDDEN,
                    "the installer must be fetched over HTTPS - use the command from `yerd remote-setup`",
                );
            }
            // Terminal step: consume the code (single-use).
            if check_code(&ctx.state, &code, true).await {
                Decision::Script
            } else {
                Decision::Text(StatusCode::FORBIDDEN, "invalid code")
            }
        }
        pure::Route::NotFound => Decision::Text(StatusCode::NOT_FOUND, "not found"),
    }
}

async fn handle_request(
    req: &Request<hyper::body::Incoming>,
    ctx: &SetupContext,
    tls: bool,
) -> Response<Full<Bytes>> {
    let decision = decide(
        ctx,
        req.method() == Method::GET,
        req.uri().path(),
        req.uri().query(),
        tls,
    )
    .await;
    match decision {
        Decision::Ca => bytes(StatusCode::OK, "application/x-pem-file", ctx.ca_pem.clone()),
        Decision::Script => {
            let script = pure::installer_script(ctx.server_ip, &ctx.tld, ctx.dns_port);
            bytes(StatusCode::OK, "text/x-shellscript", script.into_bytes())
        }
        Decision::Text(status, msg) => text(status, msg),
    }
}

/// Check `candidate` against the live one-time code. `consume` marks it used on a
/// match (the terminal script fetch). A wrong guess increments the attempt
/// counter and invalidates the code past [`MAX_CODE_ATTEMPTS`].
async fn check_code(state: &DaemonState, candidate: &str, consume: bool) -> bool {
    let now = Instant::now();
    let mut guard = state.remote_setup_code.lock().await;
    let Some(code) = guard.as_mut() else {
        return false;
    };
    if code.used || code.expires_at <= now {
        return false;
    }
    if !pure::ct_eq(&code.value, candidate) {
        code.attempts = code.attempts.saturating_add(1);
        if code.attempts >= MAX_CODE_ATTEMPTS {
            code.used = true;
        }
        return false;
    }
    if consume {
        code.used = true;
    }
    true
}

fn text(status: StatusCode, msg: &str) -> Response<Full<Bytes>> {
    bytes(status, "text/plain", msg.as_bytes().to_vec())
}

fn bytes(status: StatusCode, content_type: &str, body: Vec<u8>) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header("content-type", content_type)
        .body(Full::new(Bytes::from(body)))
        .unwrap_or_else(|_| Response::new(Full::new(Bytes::new())))
}

/// Pure, table-tested helpers: route classification, code extraction, the
/// constant-time compare, and the device installer-script generator.
pub mod pure {
    use std::net::Ipv4Addr;

    /// The two served routes (plus a catch-all).
    #[derive(Debug, PartialEq, Eq, Clone, Copy)]
    pub enum Route {
        /// `GET /remote-setup/ca` - the public CA PEM (HTTP + HTTPS).
        Ca,
        /// `GET /remote-setup` - the installer script (HTTPS only).
        Script,
        /// Anything else.
        NotFound,
    }

    /// Classify a request path into a [`Route`]. Fixed paths only - request input
    /// is never joined to a filesystem path.
    #[must_use]
    pub fn classify(path: &str) -> Route {
        match path {
            "/remote-setup/ca" => Route::Ca,
            "/remote-setup" => Route::Script,
            _ => Route::NotFound,
        }
    }

    /// Extract the `code` query parameter, if present and non-empty.
    #[must_use]
    pub fn extract_code(query: Option<&str>) -> Option<String> {
        let q = query?;
        for pair in q.split('&') {
            if let Some(v) = pair.strip_prefix("code=") {
                if !v.is_empty() {
                    return Some(v.to_owned());
                }
            }
        }
        None
    }

    /// Constant-time string compare (length-independent early-out only on a
    /// length mismatch, which is not secret - the code length is fixed).
    #[must_use]
    pub fn ct_eq(a: &str, b: &str) -> bool {
        use subtle::ConstantTimeEq as _;
        let (a, b) = (a.as_bytes(), b.as_bytes());
        if a.len() != b.len() {
            return false;
        }
        a.ct_eq(b).into()
    }

    /// The device-side installer script (served over fingerprint-anchored
    /// HTTPS). It requires the CA fingerprint as `$1` (the trust anchor,
    /// copy-pasted), re-verifies the already-downloaded `yerd-ca.pem` against it
    /// (DER SHA-256, never a PEM-file hash), then installs the CA into the
    /// device trust store and points the `.test` resolver at the host. `$2 ==
    /// uninstall` reverses it. Values are numeric/validated, so no shell
    /// injection surface.
    #[must_use]
    pub fn installer_script(server_ip: Ipv4Addr, tld: &str, dns_port: u16) -> String {
        INSTALLER_TEMPLATE
            .replace("@TLD@", tld)
            .replace("@SERVER_IP@", &server_ip.to_string())
            .replace("@DNS_PORT@", &dns_port.to_string())
    }

    const INSTALLER_TEMPLATE: &str = r#"#!/usr/bin/env bash
set -euo pipefail

FP="${1:-}"
MODE="${2:-install}"
TLD="@TLD@"
SERVER_IP="@SERVER_IP@"
DNS_PORT="@DNS_PORT@"
CA="yerd-ca.pem"

if [ -z "$FP" ]; then
  echo "usage: sudo bash yerd-setup.sh <ca-fingerprint> [uninstall]" >&2
  exit 2
fi

verify_ca() {
  if [ ! -f "$CA" ]; then
    echo "error: $CA not found next to this script (re-run the full command from 'yerd remote-setup')" >&2
    exit 1
  fi
  got="$(openssl x509 -in "$CA" -noout -fingerprint -sha256 | sed 's/.*=//;s/://g' | tr 'A-Z' 'a-z')"
  if [ "$got" != "$FP" ]; then
    echo "error: CA fingerprint mismatch - refusing to install (expected $FP, got $got)" >&2
    exit 1
  fi
}

os="$(uname -s)"

case "$MODE" in
install)
  verify_ca
  case "$os" in
  Darwin)
    security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain "$CA"
    mkdir -p /etc/resolver
    printf 'nameserver %s\nport %s\n' "$SERVER_IP" "$DNS_PORT" > "/etc/resolver/$TLD"
    echo "Installed. .$TLD now resolves via $SERVER_IP (DNS $DNS_PORT)."
    ;;
  Linux)
    if command -v update-ca-certificates >/dev/null 2>&1; then
      cp "$CA" "/usr/local/share/ca-certificates/yerd-$TLD.crt"
      update-ca-certificates >/dev/null
    elif command -v update-ca-trust >/dev/null 2>&1; then
      cp "$CA" "/etc/pki/ca-trust/source/anchors/yerd-$TLD.pem"
      update-ca-trust
    else
      echo "error: no CA trust tool found (update-ca-certificates / update-ca-trust)" >&2
      exit 1
    fi
    if [ -d /etc/NetworkManager/dnsmasq.d ]; then
      printf 'server=/%s/%s#%s\n' "$TLD" "$SERVER_IP" "$DNS_PORT" > "/etc/NetworkManager/dnsmasq.d/yerd-$TLD.conf"
      systemctl reload NetworkManager 2>/dev/null || true
    elif [ -d /etc/dnsmasq.d ]; then
      printf 'server=/%s/%s#%s\n' "$TLD" "$SERVER_IP" "$DNS_PORT" > "/etc/dnsmasq.d/yerd-$TLD.conf"
      systemctl restart dnsmasq 2>/dev/null || true
    else
      echo "error: unsupported resolver setup - install dnsmasq or use NetworkManager." >&2
      echo "       (systemd-resolved alone cannot forward a single domain to a custom port.)" >&2
      exit 1
    fi
    echo "Installed. .$TLD now resolves via $SERVER_IP (DNS $DNS_PORT)."
    ;;
  *)
    echo "error: unsupported OS: $os" >&2
    exit 1
    ;;
  esac
  ;;
uninstall)
  case "$os" in
  Darwin)
    rm -f "/etc/resolver/$TLD"
    security delete-certificate -c "Yerd Local CA" /Library/Keychains/System.keychain 2>/dev/null || true
    ;;
  Linux)
    rm -f "/usr/local/share/ca-certificates/yerd-$TLD.crt" "/etc/pki/ca-trust/source/anchors/yerd-$TLD.pem"
    rm -f "/etc/NetworkManager/dnsmasq.d/yerd-$TLD.conf" "/etc/dnsmasq.d/yerd-$TLD.conf"
    if command -v update-ca-certificates >/dev/null 2>&1; then update-ca-certificates --fresh >/dev/null 2>&1 || true; fi
    if command -v update-ca-trust >/dev/null 2>&1; then update-ca-trust 2>/dev/null || true; fi
    ;;
  esac
  echo "Uninstalled Yerd LAN setup for .$TLD."
  ;;
*)
  echo "usage: sudo bash yerd-setup.sh <ca-fingerprint> [uninstall]" >&2
  exit 2
  ;;
esac
"#;

    #[cfg(test)]
    #[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    mod tests {
        use super::*;

        #[test]
        fn classify_fixed_routes() {
            assert_eq!(classify("/remote-setup"), Route::Script);
            assert_eq!(classify("/remote-setup/ca"), Route::Ca);
            assert_eq!(classify("/"), Route::NotFound);
            assert_eq!(classify("/remote-setup/../etc"), Route::NotFound);
        }

        #[test]
        fn extract_code_parses_param() {
            assert_eq!(extract_code(Some("code=abc")).as_deref(), Some("abc"));
            assert_eq!(extract_code(Some("x=1&code=abc")).as_deref(), Some("abc"));
            assert_eq!(extract_code(Some("code=")), None);
            assert_eq!(extract_code(Some("nope=1")), None);
            assert_eq!(extract_code(None), None);
        }

        #[test]
        fn ct_eq_matches_only_equal() {
            assert!(ct_eq("deadbeef", "deadbeef"));
            assert!(!ct_eq("deadbeef", "deadbee0"));
            assert!(!ct_eq("deadbeef", "deadbee"));
        }

        #[test]
        fn installer_script_interpolates_and_verifies_der_fingerprint() {
            let s = installer_script(Ipv4Addr::new(192, 168, 1, 42), "test", 1053);
            assert!(s.contains("SERVER_IP=\"192.168.1.42\""));
            assert!(s.contains("TLD=\"test\""));
            assert!(s.contains("DNS_PORT=\"1053\""));
            // DER fingerprint verify, not a PEM-file hash (R3-M1).
            assert!(s.contains("openssl x509 -in \"$CA\" -noout -fingerprint -sha256"));
            assert!(!s.contains("shasum"), "must not hash the PEM file");
            // Per-OS branches + systemd-resolved-alone refusal.
            assert!(s.contains("Darwin)"));
            assert!(s.contains("/etc/resolver/$TLD"));
            assert!(s.contains("server=/%s/%s#%s"));
            assert!(s.contains("systemd-resolved alone cannot forward"));
            // Uninstall mode.
            assert!(s.contains("uninstall)"));
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod endpoint_tests {
    use std::time::Duration;

    use super::*;
    use crate::state::RemoteSetupCode;
    use crate::test_support::state_in;

    fn ctx_with_code(state: Arc<DaemonState>) -> SetupContext {
        SetupContext {
            ca_pem: b"-----BEGIN CERTIFICATE-----\nX\n-----END CERTIFICATE-----\n".to_vec(),
            tld: "test".into(),
            dns_port: 1053,
            server_ip: Ipv4Addr::new(192, 168, 1, 42),
            state,
        }
    }

    async fn seed(state: &DaemonState, value: &str) {
        *state.remote_setup_code.lock().await = Some(RemoteSetupCode {
            value: value.to_owned(),
            expires_at: Instant::now() + Duration::from_secs(60),
            used: false,
            attempts: 0,
        });
    }

    #[tokio::test]
    async fn ca_route_needs_valid_code_but_does_not_consume() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(state_in(tmp.path()));
        seed(&state, "good").await;
        let ctx = ctx_with_code(Arc::clone(&state));

        // Bad code → 403.
        assert!(matches!(
            decide(&ctx, true, "/remote-setup/ca", Some("code=bad"), false).await,
            Decision::Text(StatusCode::FORBIDDEN, _)
        ));
        // Good code → CA, and it stays usable (not consumed).
        assert_eq!(
            decide(&ctx, true, "/remote-setup/ca", Some("code=good"), false).await,
            Decision::Ca
        );
        assert_eq!(
            decide(&ctx, true, "/remote-setup/ca", Some("code=good"), false).await,
            Decision::Ca
        );
    }

    #[tokio::test]
    async fn script_route_requires_https_and_is_single_use() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(state_in(tmp.path()));
        seed(&state, "good").await;
        let ctx = ctx_with_code(Arc::clone(&state));

        // Plaintext script fetch is refused.
        assert!(matches!(
            decide(&ctx, true, "/remote-setup", Some("code=good"), false).await,
            Decision::Text(StatusCode::FORBIDDEN, _)
        ));
        // Over TLS with the right code → script, and it consumes the code.
        assert_eq!(
            decide(&ctx, true, "/remote-setup", Some("code=good"), true).await,
            Decision::Script
        );
        // Replay is rejected (single-use).
        assert!(matches!(
            decide(&ctx, true, "/remote-setup", Some("code=good"), true).await,
            Decision::Text(StatusCode::FORBIDDEN, _)
        ));
    }

    #[tokio::test]
    async fn method_and_path_guards() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(state_in(tmp.path()));
        seed(&state, "good").await;
        let ctx = ctx_with_code(Arc::clone(&state));

        assert!(matches!(
            decide(&ctx, false, "/remote-setup/ca", Some("code=good"), false).await,
            Decision::Text(StatusCode::METHOD_NOT_ALLOWED, _)
        ));
        assert!(matches!(
            decide(&ctx, true, "/nope", Some("code=good"), true).await,
            Decision::Text(StatusCode::NOT_FOUND, _)
        ));
        assert!(matches!(
            decide(&ctx, true, "/remote-setup/ca", None, false).await,
            Decision::Text(StatusCode::FORBIDDEN, _)
        ));
    }

    #[tokio::test]
    async fn wrong_code_lockout_invalidates() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(state_in(tmp.path()));
        seed(&state, "good").await;
        let ctx = ctx_with_code(Arc::clone(&state));

        for _ in 0..MAX_CODE_ATTEMPTS {
            let _ = decide(&ctx, true, "/remote-setup/ca", Some("code=bad"), false).await;
        }
        // After the lockout the correct code no longer works.
        assert!(matches!(
            decide(&ctx, true, "/remote-setup/ca", Some("code=good"), false).await,
            Decision::Text(StatusCode::FORBIDDEN, _)
        ));
    }
}
