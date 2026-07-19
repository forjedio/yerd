//! LAN remote-device bootstrap endpoint.
//!
//! A small hyper service on `0.0.0.0:<lan_setup_port>`, spawned only while LAN
//! mode is on and a LAN IP was discovered. It implements a **hash-anchored**
//! trust flow:
//!
//! 1. The device fetches one **self-contained installer script** over plain HTTP
//!    (`GET /remote-setup?code=…`). The script embeds the public CA PEM inline,
//!    so there is nothing else to download and no live trust needed yet.
//! 2. Before running it, the operator's pasted command verifies the script's
//!    **SHA-256** equals the hash printed by `yerd remote-setup` (copied out of
//!    band, never trusted from the wire). That hash covers the embedded CA and
//!    the resolver config, so a tampered script is rejected.
//! 3. The verified script installs the embedded CA into the device trust store
//!    and points the `.test` resolver at the host.
//!
//! The endpoint is HTTP-only: content integrity comes from the pasted hash, not
//! from transport security, so no TLS leaf is minted here. The CA **private key
//! never leaves the daemon**; only the public CA cert is embedded in the script.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::{Duration, Instant};

use http_body_util::Full;
use hyper::body::Bytes;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{watch, Semaphore};

use crate::state::DaemonState;

/// Immutable context the endpoint serves from, built once at spawn.
pub struct SetupContext {
    /// The fully rendered installer script bytes, hashed once at spawn so the
    /// SHA-256 printed by `yerd remote-setup` always matches what is served.
    pub script: Vec<u8>,
    /// Shared daemon state, for the one-time code store.
    pub state: Arc<DaemonState>,
}

/// Cap on concurrent bootstrap connections, so a peer can't exhaust the daemon
/// by opening many slow connections. Small: a real bootstrap is a couple of
/// short-lived fetches.
const MAX_CONNS: usize = 32;

/// End-to-end deadline for one connection (one request), so a stalled/slow-loris
/// connection is dropped rather than held open.
const CONN_TIMEOUT: Duration = Duration::from_secs(15);

/// Serve until `shutdown` resolves. Non-LAN peers are dropped at accept.
pub async fn serve(
    listener: TcpListener,
    ctx: Arc<SetupContext>,
    mut shutdown: watch::Receiver<bool>,
) {
    let limit = Arc::new(Semaphore::new(MAX_CONNS));
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
                if !yerd_core::is_lan_source(peer.ip()) {
                    continue;
                }
                let Ok(permit) = Arc::clone(&limit).try_acquire_owned() else {
                    tracing::debug!("lan setup: connection cap reached, dropping");
                    continue;
                };
                let ctx = Arc::clone(&ctx);
                tokio::spawn(async move {
                    let _permit = permit;
                    let _ = tokio::time::timeout(CONN_TIMEOUT, serve_conn(stream, ctx)).await;
                });
            }
            _ = shutdown.changed() => break,
        }
    }
}

async fn serve_conn(stream: TcpStream, ctx: Arc<SetupContext>) {
    let service = service_fn(move |req| {
        let ctx = Arc::clone(&ctx);
        async move { Ok::<_, Infallible>(handle_request(&req, &ctx).await) }
    });
    if let Err(e) = hyper::server::conn::http1::Builder::new()
        .serve_connection(TokioIo::new(stream), service)
        .await
    {
        tracing::debug!(error = %e, "lan setup: connection error");
    }
}

/// What the endpoint decided to reply with, independent of the hyper types so it
/// can be unit-tested against a seeded [`DaemonState`].
#[derive(Debug, PartialEq, Eq)]
enum Decision {
    /// Serve the installer script.
    Script,
    /// A plain-text status reply (error / not-found).
    Text(StatusCode, &'static str),
}

/// Route the request. The single script route requires the one-time code
/// (single-use-consumed). Content integrity is guaranteed by the SHA-256 the
/// operator verifies before running, not by the code or the transport, so the
/// code is only an authorization gate on who may fetch the installer.
async fn decide(ctx: &SetupContext, is_get: bool, path: &str, query: Option<&str>) -> Decision {
    if !is_get {
        return Decision::Text(StatusCode::METHOD_NOT_ALLOWED, "GET only");
    }
    match pure::classify(path) {
        pure::Route::Script => {
            let Some(code) = pure::extract_code(query) else {
                return Decision::Text(StatusCode::FORBIDDEN, "missing code");
            };
            if consume_code(&ctx.state, &code).await {
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
) -> Response<Full<Bytes>> {
    let decision = decide(
        ctx,
        req.method() == Method::GET,
        req.uri().path(),
        req.uri().query(),
    )
    .await;
    match decision {
        Decision::Script => bytes(StatusCode::OK, "text/x-shellscript", ctx.script.clone()),
        Decision::Text(status, msg) => text(status, msg),
    }
}

/// Constant-time-compare `candidate` against the live one-time code and, on a
/// match, mark it used (single-use). A mismatch is a plain rejection with **no**
/// mutation, so an unauthenticated peer's wrong guesses cannot revoke the
/// freshly minted code (the 128-bit code makes brute-force infeasible without a
/// lockout).
async fn consume_code(state: &DaemonState, candidate: &str) -> bool {
    let now = Instant::now();
    let mut guard = state.remote_setup_code.lock().await;
    let Some(code) = guard.as_mut() else {
        return false;
    };
    if code.used || code.expires_at <= now || !pure::ct_eq(&code.value, candidate) {
        return false;
    }
    code.used = true;
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

    /// The served route (plus a catch-all).
    #[derive(Debug, PartialEq, Eq, Clone, Copy)]
    pub enum Route {
        /// `GET /remote-setup` - the self-contained installer script.
        Script,
        /// Anything else.
        NotFound,
    }

    /// Classify a request path into a [`Route`]. Fixed paths only - request input
    /// is never joined to a filesystem path.
    #[must_use]
    pub fn classify(path: &str) -> Route {
        match path {
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

    /// The self-contained device installer script. The public CA PEM is embedded
    /// inline (`@CA_PEM@`), so the device downloads nothing else and trust comes
    /// entirely from the caller having verified this script's SHA-256 before
    /// running it. It installs the embedded CA into the OS trust store **and**,
    /// on Linux, the desktop user's NSS databases (so Firefox / Chromium / Brave
    /// trust it), then points the `.test` resolver at the host. `$1 == uninstall`
    /// reverses it. Interpolated values are numeric/validated, so there is no
    /// shell-injection surface.
    #[must_use]
    pub fn installer_script(server_ip: Ipv4Addr, tld: &str, dns_port: u16, ca_pem: &str) -> String {
        INSTALLER_TEMPLATE
            .replace("@TLD@", tld)
            .replace("@SERVER_IP@", &server_ip.to_string())
            .replace("@DNS_PORT@", &dns_port.to_string())
            .replace("@CA_PEM@", ca_pem.trim_end())
    }

    const INSTALLER_TEMPLATE: &str = r#"#!/usr/bin/env bash
set -euo pipefail

MODE="${1:-install}"
TLD="@TLD@"
SERVER_IP="@SERVER_IP@"
DNS_PORT="@DNS_PORT@"
NSS_NICK="Yerd Local CA ($TLD)"

# The Yerd CA is embedded below and authenticated by the SHA-256 you verified
# before running this script, so there is nothing else to download or trust.
read_ca() {
  cat <<'YERD_CA_PEM_EOF'
@CA_PEM@
YERD_CA_PEM_EOF
}

# Resolve the desktop user (this runs under sudo, so $HOME is root's - the
# browser NSS stores live in the invoking user's home instead).
desktop_home() {
  [ -n "${SUDO_USER:-}" ] || return 1
  getent passwd "$SUDO_USER" 2>/dev/null | cut -d: -f6
}

# Add/refresh the CA in the desktop user's NSS databases so Firefox, Chromium
# and Brave trust it. Best-effort: skipped silently when certutil or the DBs are
# absent, and never fatal to the system-store install.
nss_install() {
  command -v certutil >/dev/null 2>&1 || return 0
  local ca="$1" user="${SUDO_USER:-}" home
  [ -n "$user" ] || return 0
  home="$(desktop_home)" || return 0
  [ -n "$home" ] || return 0

  local shared="$home/.pki/nssdb"
  if [ ! -d "$shared" ]; then
    sudo -u "$user" mkdir -p "$shared" >/dev/null 2>&1 || true
    sudo -u "$user" certutil -d "sql:$shared" -N --empty-password >/dev/null 2>&1 || true
  fi
  for db in "$shared" \
            "$home"/.mozilla/firefox/*/ \
            "$home"/snap/firefox/common/.mozilla/firefox/*/ \
            "$home"/.var/app/org.mozilla.firefox/.mozilla/firefox/*/; do
    [ -d "$db" ] || continue
    [ "$db" = "$shared" ] || [ -f "$db/cert9.db" ] || [ -f "$db/cert8.db" ] || continue
    sudo -u "$user" certutil -d "sql:$db" -D -n "$NSS_NICK" >/dev/null 2>&1 || true
    sudo -u "$user" certutil -d "sql:$db" -A -t C,, -n "$NSS_NICK" -i "$ca" >/dev/null 2>&1 || true
  done
}

nss_uninstall() {
  command -v certutil >/dev/null 2>&1 || return 0
  local user="${SUDO_USER:-}" home
  [ -n "$user" ] || return 0
  home="$(desktop_home)" || return 0
  [ -n "$home" ] || return 0
  for db in "$home/.pki/nssdb" \
            "$home"/.mozilla/firefox/*/ \
            "$home"/snap/firefox/common/.mozilla/firefox/*/ \
            "$home"/.var/app/org.mozilla.firefox/.mozilla/firefox/*/; do
    [ -d "$db" ] || continue
    sudo -u "$user" certutil -d "sql:$db" -D -n "$NSS_NICK" >/dev/null 2>&1 || true
  done
}

os="$(uname -s)"
CA_TMP=""
cleanup_tmp() { [ -n "$CA_TMP" ] && rm -f "$CA_TMP"; }

case "$MODE" in
install)
  CA_TMP="$(mktemp)"
  trap cleanup_tmp EXIT
  read_ca > "$CA_TMP"
  # World-readable so certutil, run as the desktop user via `sudo -u`, can read
  # it for the NSS import - mktemp makes it 0600/root by default. The CA is
  # public, and the file is deleted on exit.
  chmod 0644 "$CA_TMP"
  case "$os" in
  Darwin)
    security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain "$CA_TMP"
    mkdir -p /etc/resolver
    printf 'nameserver %s\nport %s\n' "$SERVER_IP" "$DNS_PORT" > "/etc/resolver/$TLD"
    echo "Installed. .$TLD now resolves via $SERVER_IP (DNS $DNS_PORT)."
    ;;
  Linux)
    # Decide the resolver target BEFORE touching the trust store, so an
    # unsupported host fails without leaving a stranded CA behind.
    if [ -d /etc/NetworkManager/dnsmasq.d ]; then
      RESOLVER_CONF="/etc/NetworkManager/dnsmasq.d/yerd-$TLD.conf"
      RESOLVER_RELOAD="systemctl reload NetworkManager"
    elif [ -d /etc/dnsmasq.d ]; then
      RESOLVER_CONF="/etc/dnsmasq.d/yerd-$TLD.conf"
      RESOLVER_RELOAD="systemctl restart dnsmasq"
    else
      echo "error: unsupported resolver setup - install dnsmasq or use NetworkManager." >&2
      echo "       (systemd-resolved alone cannot forward a single domain to a custom port.)" >&2
      exit 1
    fi
    # Pick by anchor *directory*, not by tool name: Arch ships update-ca-trust
    # as a compat shim while its anchors live elsewhere, so probing the tool
    # first picks a destination directory that does not exist.
    if [ -d /usr/local/share/ca-certificates ] && command -v update-ca-certificates >/dev/null 2>&1; then
      CA_DEST="/usr/local/share/ca-certificates/yerd-$TLD.crt"; CA_REFRESH="update-ca-certificates"
    elif [ -d /etc/pki/ca-trust/source/anchors ] && command -v update-ca-trust >/dev/null 2>&1; then
      CA_DEST="/etc/pki/ca-trust/source/anchors/yerd-$TLD.pem"; CA_REFRESH="update-ca-trust extract"
    elif [ -d /etc/ca-certificates/trust-source/anchors ] && command -v trust >/dev/null 2>&1; then
      CA_DEST="/etc/ca-certificates/trust-source/anchors/yerd-$TLD.crt"; CA_REFRESH="trust extract-compat"
    else
      echo "error: no usable CA anchor directory found (expected Debian/Ubuntu, RHEL/Fedora or Arch layout)" >&2
      exit 1
    fi
    # Roll the CA back if the resolver step below fails, so we don't leave the
    # device trusting a CA whose .test names it can't resolve.
    cp "$CA_TMP" "$CA_DEST"
    trap 'rm -f "$CA_DEST"; $CA_REFRESH >/dev/null 2>&1 || true; cleanup_tmp' EXIT
    $CA_REFRESH >/dev/null
    printf 'server=/%s/%s#%s\n' "$TLD" "$SERVER_IP" "$DNS_PORT" > "$RESOLVER_CONF"
    $RESOLVER_RELOAD 2>/dev/null || true
    trap cleanup_tmp EXIT
    nss_install "$CA_TMP"
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
    # Delete each matching Yerd CA by its exact SHA-1 hash (not just the common
    # name), clearing that certificate's trust settings too.
    security find-certificate -a -Z -c "Yerd Local CA" /Library/Keychains/System.keychain 2>/dev/null \
      | awk '/SHA-1 hash:/ {print $NF}' \
      | while read -r h; do
          [ -n "$h" ] || continue
          security remove-trusted-cert -d -Z "$h" 2>/dev/null || true
          security delete-certificate -Z "$h" /Library/Keychains/System.keychain 2>/dev/null || true
        done
    ;;
  Linux)
    rm -f "/usr/local/share/ca-certificates/yerd-$TLD.crt" \
          "/etc/pki/ca-trust/source/anchors/yerd-$TLD.pem" \
          "/etc/ca-certificates/trust-source/anchors/yerd-$TLD.crt"
    rm -f "/etc/NetworkManager/dnsmasq.d/yerd-$TLD.conf" "/etc/dnsmasq.d/yerd-$TLD.conf"
    if command -v update-ca-certificates >/dev/null 2>&1; then update-ca-certificates --fresh >/dev/null 2>&1 || true; fi
    if command -v update-ca-trust >/dev/null 2>&1; then update-ca-trust extract >/dev/null 2>&1 || true; fi
    if command -v trust >/dev/null 2>&1; then trust extract-compat >/dev/null 2>&1 || true; fi
    nss_uninstall
    ;;
  esac
  echo "Uninstalled Yerd LAN setup for .$TLD."
  ;;
*)
  echo "usage: sudo bash yerd-setup.sh [uninstall]" >&2
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
            assert_eq!(classify("/remote-setup/ca"), Route::NotFound);
            assert_eq!(classify("/"), Route::NotFound);
            assert_eq!(classify("/remote-setup/../etc"), Route::NotFound);
        }

        const SAMPLE_CA: &str =
            "-----BEGIN CERTIFICATE-----\nMIIByerdSAMPLE\n-----END CERTIFICATE-----\n";

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
        fn installer_script_interpolates_and_embeds_the_ca() {
            let s = installer_script(Ipv4Addr::new(192, 168, 1, 42), "test", 1053, SAMPLE_CA);
            assert!(s.contains("SERVER_IP=\"192.168.1.42\""));
            assert!(s.contains("TLD=\"test\""));
            assert!(s.contains("DNS_PORT=\"1053\""));
            assert!(
                s.contains("MIIByerdSAMPLE"),
                "the CA is embedded inline, not downloaded separately"
            );
            assert!(
                !s.contains("yerd-ca.pem") && !s.contains("openssl x509"),
                "there is no separate CA file to fetch or fingerprint - the outer hash covers it"
            );
            assert!(s.contains("Darwin)"));
            assert!(s.contains("/etc/resolver/$TLD"));
            assert!(s.contains("server=/%s/%s#%s"));
            assert!(s.contains("systemd-resolved alone cannot forward"));
            assert!(s.contains("uninstall)"));
        }

        #[test]
        fn installer_script_installs_the_ca_into_nss_for_browsers() {
            let s = installer_script(Ipv4Addr::new(10, 0, 0, 5), "test", 1053, SAMPLE_CA);
            assert!(s.contains("certutil"), "NSS install uses certutil");
            assert!(
                s.contains(".mozilla/firefox") && s.contains(".pki/nssdb"),
                "covers both Firefox profiles and the Chromium/Brave shared NSS DB"
            );
            assert!(
                s.contains("sudo -u \"$user\""),
                "NSS DBs belong to the desktop user, not root"
            );
            assert!(
                s.contains("nss_uninstall"),
                "uninstall removes the CA from NSS too"
            );
        }

        #[test]
        fn installer_script_linux_checks_resolver_before_installing_ca_and_rolls_back() {
            let s = installer_script(Ipv4Addr::new(10, 0, 0, 5), "test", 1053, SAMPLE_CA);
            let resolver_check = s
                .find("unsupported resolver setup")
                .expect("resolver support is validated");
            let ca_copy = s.find("cp \"$CA_TMP\"").expect("CA is installed");
            assert!(
                resolver_check < ca_copy,
                "resolver support must be checked before the CA is installed"
            );
            assert!(
                s.contains("trap 'rm -f \"$CA_DEST\""),
                "the CA is rolled back if the resolver step fails"
            );
        }

        #[test]
        fn installer_script_picks_the_linux_anchor_dir_by_directory_not_by_tool() {
            let s = installer_script(Ipv4Addr::new(10, 0, 0, 5), "test", 1053, SAMPLE_CA);
            for dir in [
                "/usr/local/share/ca-certificates",
                "/etc/pki/ca-trust/source/anchors",
                "/etc/ca-certificates/trust-source/anchors",
            ] {
                assert!(
                    s.contains(&format!("[ -d {dir} ]")),
                    "{dir} is probed before it is written to"
                );
            }
            assert!(
                s.contains("trust extract-compat"),
                "Arch refreshes via p11-kit, not update-ca-trust"
            );
            let probe = s
                .find("[ -d /usr/local/share/ca-certificates ]")
                .expect("anchor dirs are probed");
            let copy = s.find("cp \"$CA_TMP\"").expect("CA is installed");
            assert!(
                probe < copy,
                "the destination dir is probed before the copy"
            );
        }

        #[test]
        fn installer_script_uninstall_deletes_the_ca_by_hash_not_common_name() {
            let s = installer_script(Ipv4Addr::new(10, 0, 0, 5), "test", 1053, SAMPLE_CA);
            assert!(s.contains("SHA-1 hash:"), "identifies the cert by its hash");
            assert!(
                s.contains("security delete-certificate -Z"),
                "deletes by hash, not `-c <common-name>`"
            );
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
            script: b"#!/usr/bin/env bash\n".to_vec(),
            state,
        }
    }

    async fn seed(state: &DaemonState, value: &str) {
        *state.remote_setup_code.lock().await = Some(RemoteSetupCode {
            value: value.to_owned(),
            expires_at: Instant::now() + Duration::from_secs(60),
            used: false,
        });
    }

    #[tokio::test]
    async fn script_route_requires_a_code_and_is_single_use() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(state_in(tmp.path()));
        seed(&state, "good").await;
        let ctx = ctx_with_code(Arc::clone(&state));

        assert!(
            matches!(
                decide(&ctx, true, "/remote-setup", None).await,
                Decision::Text(StatusCode::FORBIDDEN, _)
            ),
            "a script fetch without a code is refused"
        );
        assert_eq!(
            decide(&ctx, true, "/remote-setup", Some("code=good")).await,
            Decision::Script
        );
        assert!(
            matches!(
                decide(&ctx, true, "/remote-setup", Some("code=good")).await,
                Decision::Text(StatusCode::FORBIDDEN, _)
            ),
            "replay is rejected (single-use)"
        );
    }

    #[tokio::test]
    async fn method_and_path_guards() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(state_in(tmp.path()));
        seed(&state, "good").await;
        let ctx = ctx_with_code(Arc::clone(&state));

        assert!(matches!(
            decide(&ctx, false, "/remote-setup", Some("code=good")).await,
            Decision::Text(StatusCode::METHOD_NOT_ALLOWED, _)
        ));
        assert!(matches!(
            decide(&ctx, true, "/nope", Some("code=good")).await,
            Decision::Text(StatusCode::NOT_FOUND, _)
        ));
    }

    #[tokio::test]
    async fn wrong_guesses_do_not_revoke_the_minted_code() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(state_in(tmp.path()));
        seed(&state, "good").await;
        let ctx = ctx_with_code(Arc::clone(&state));

        for _ in 0..50 {
            let _ = decide(&ctx, true, "/remote-setup", Some("code=bad")).await;
        }
        assert_eq!(
            decide(&ctx, true, "/remote-setup", Some("code=good")).await,
            Decision::Script,
            "an unauthenticated peer's wrong guesses must not lock out the legit device"
        );
    }
}
