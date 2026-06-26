//! macOS implementations of the four traits.
//!
//! `Paths` uses `directories` for `config`/`data`/`cache`; `state`
//! coincides with `data` on macOS (no XDG state distinction); `runtime`
//! is a deterministic `/tmp/yerd-$UID` directory (see `resolve` for why
//! it is not `std::env::temp_dir()`).
//!
//! `TrustStore::is_present_system` enumerates the default Keychain
//! search list (which includes `/Library/Keychains/System.keychain`)
//! via `security-framework` and matches certificates by SHA-256 over
//! their DER body. Privileged ops return `NeedsHelper`. `bind_pair`
//! has its own copy of the impl (the Linux one is `#[cfg]`-gated to
//! Linux); the shared decision logic lives in `pure::port_plan`.

#![allow(clippy::similar_names)]

use std::fs;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::Command;

use directories::ProjectDirs;

use crate::error::ops;
use crate::metrics::SystemMetrics;
use crate::paths::{Paths, PlatformDirs};
use crate::port_binder::{BoundPort, PortBinder, PortPair};
use crate::port_redirect::{
    loopback_port_reachable, loopback_redirect_reaches_proxy, PortRedirector,
};
use crate::pure::{pem_match, port_plan, ps_metrics, resolver_file};
use crate::resolver::ResolverInstaller;
use crate::trust_store::{CaFingerprint, NssOutcome, TrustStore};
use crate::{BindPairErrorReason, PlatformError, ResolverErrorReason, TrustStoreErrorReason};

/// macOS `Paths` implementation.
#[derive(Debug, Default, Clone, Copy)]
pub struct MacosPaths;

impl MacosPaths {
    /// Construct.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Paths for MacosPaths {
    fn resolve(&self) -> Result<PlatformDirs, PlatformError> {
        let pd = ProjectDirs::from("io", "yerd", "Yerd").ok_or(PlatformError::MissingHomeDir)?;
        let config = pd.config_dir().to_path_buf();
        let data = pd.data_dir().to_path_buf();
        let cache = pd.cache_dir().to_path_buf();
        // macOS has no XDG state distinction; collapse to data.
        let state = data.clone();

        // No XDG_RUNTIME_DIR on macOS. Use a deterministic, uid-derived
        // `/tmp/yerd-$UID` (matching Linux's XDG-less fallback) rather than
        // `std::env::temp_dir()` (`$TMPDIR` = a per-session `/var/folders/…`
        // path). Determinism is load-bearing: `yerd elevate`, running as root
        // under `osascript`/`sudo`, must reconstruct this socket path from
        // `SUDO_UID` alone (see `bin/yerd/src/elevate.rs::user_socket_candidates`),
        // and it cannot read another user's `$TMPDIR` without privileged FFI
        // (forbidden in this workspace). The daemon and GUI both resolve via
        // this same function, so they always agree on the path.
        //
        // Trade-off: `/tmp` is world-traversable + sticky, so a hostile local
        // uid could pre-create `/tmp/yerd-$UID` to make the daemon's
        // fail-closed `secure_fs::create_private_dir` (0o700, owner-checked)
        // refuse to start — the same DoS surface the Linux fallback already
        // accepts. Caller must still set mode 0o700.
        //
        // Sandbox caveat: this works because the GUI `.app` is unsigned and
        // unsandboxed, so it shares the user's `/tmp` namespace with a
        // terminal-launched daemon. If the app is ever signed + sandboxed,
        // `temp_dir()`/`/tmp` access becomes a container path and GUI↔daemon
        // IPC over this socket would break — revisit then.
        let uid = read_real_uid().unwrap_or(0);
        let runtime = PathBuf::from(format!("/tmp/yerd-{uid}"));

        Ok(PlatformDirs {
            config,
            data,
            state,
            cache,
            runtime,
        })
    }
}

/// Read the real UID via `/usr/bin/id -u`, which is present on every macOS
/// install. `std::process::Command` is acceptable here because (a) the input is
/// constant, (b) the output is parsed as a `u32`, (c) no privilege boundary is
/// crossed.
///
/// **The path must be absolute.** When the daemon is launched by
/// launchd/SMAppService its `PATH` is minimal and need not contain `/usr/bin`,
/// so a bare `id` would fail to exec → `None` → the caller's `unwrap_or(0)` would
/// silently bind the socket under `/tmp/yerd-0` while the GUI (full login `PATH`)
/// resolves `/tmp/yerd-$realuid` — the daemon then looks "unreachable" though it
/// is healthy. Matching the `/bin/ps` call below keeps this deterministic under
/// the service manager's stripped environment.
fn read_real_uid() -> Option<u32> {
    let out = std::process::Command::new("/usr/bin/id")
        .arg("-u")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    s.trim().parse().ok()
}

/// macOS `TrustStore` implementation.
#[derive(Debug, Default, Clone, Copy)]
pub struct MacosTrustStore;

impl MacosTrustStore {
    /// Construct.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl TrustStore for MacosTrustStore {
    fn install_system(&self, _: &str, _: &CaFingerprint) -> Result<(), PlatformError> {
        Err(PlatformError::NeedsHelper {
            operation: ops::INSTALL_CA,
        })
    }

    fn uninstall_system(&self, _: &CaFingerprint) -> Result<(), PlatformError> {
        Err(PlatformError::NeedsHelper {
            operation: ops::UNINSTALL_CA,
        })
    }

    fn is_present_system(&self, fp: &CaFingerprint) -> Result<bool, PlatformError> {
        use security_framework::item::{ItemClass, ItemSearchOptions, Reference, SearchResult};

        let mut opts = ItemSearchOptions::new();
        opts.class(ItemClass::certificate());
        opts.load_refs(true);
        opts.limit(10_000);

        let results = opts.search().map_err(|e| PlatformError::TrustStore {
            reason: TrustStoreErrorReason::SystemApi(e.to_string()),
        })?;

        for result in results {
            if let SearchResult::Ref(Reference::Certificate(cert)) = result {
                let der = cert.to_der();
                if pem_match::sha256(&der) == *fp.as_bytes() {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    fn is_trusted(&self, ca_path: &Path, _fp: &CaFingerprint) -> Result<bool, PlatformError> {
        use security_framework::certificate::SecCertificate;
        use security_framework::trust_settings::{
            Domain, TrustSettings, TrustSettingsForCertificate,
        };

        // Read the cert's *stored trust settings* in the user and admin domains
        // — NOT `security verify-cert`. verify-cert reflects `trustd`'s effective
        // evaluation, which is cached and can serve a stale "trusted" result
        // after the trust setting is removed (observed: it survives even
        // `killall trustd`). Reading the settings store reflects the actual
        // configuration immediately. The `Ok(None)`-means-trust ambiguity the
        // crate documents only applies to Apple's built-in *system-store* roots;
        // a cert we trust via `set_trust_settings_always` / `add-trusted-cert
        // -r trustRoot` reads back as `Some(TrustRoot)`, so `None`/not-found here
        // unambiguously means "not trusted in this domain".
        let pem = fs::read(ca_path).map_err(|source| PlatformError::Io {
            path: ca_path.to_path_buf(),
            source,
        })?;
        let der = pem_match::first_cert_der(&pem).ok_or_else(|| PlatformError::TrustStore {
            reason: TrustStoreErrorReason::SystemApi("CA PEM has no certificate".to_owned()),
        })?;
        let cert = SecCertificate::from_der(&der).map_err(|e| PlatformError::TrustStore {
            reason: TrustStoreErrorReason::SystemApi(format!("parse CA der: {e}")),
        })?;

        let trusted_in = |domain: Domain| -> Result<bool, PlatformError> {
            match TrustSettings::new(domain).tls_trust_settings_for_certificate(&cert) {
                // The presence of a (non-Deny) trust record means we trusted this
                // CA. Crucially, `set_trust_settings_always` / `add-trusted-cert
                // -r trustRoot` write an *empty* settings array ("always trust as
                // root"), which this API surfaces as `Ok(None)` (its loop over the
                // empty array never runs) — so `Ok(None)` here means **trusted**,
                // not untrusted. The function only ever yields `Some(TrustRoot)`,
                // `Some(TrustAsRoot)`, `Some(Deny)`, or `None` (it filters
                // Unspecified/Invalid to `None`), so an explicit SSL `Deny` is the
                // only "has a record but not trusted" case.
                Ok(Some(TrustSettingsForCertificate::Deny)) => Ok(false),
                Ok(_) => Ok(true),
                // errSecItemNotFound (-25300): no record for this cert in this
                // domain → not trusted there (treat as `false`, don't propagate).
                Err(e) if e.code() == -25300 => Ok(false),
                Err(e) => Err(PlatformError::TrustStore {
                    reason: TrustStoreErrorReason::SystemApi(format!("trust settings: {e}")),
                }),
            }
        };
        Ok(trusted_in(Domain::User)? || trusted_in(Domain::Admin)?)
    }

    fn install_firefox_nss(&self, _: &str) -> Result<NssOutcome, PlatformError> {
        // Phase 1: report not-attempted via a degraded outcome with
        // certutil_missing = true (it usually is on macOS without
        // the Homebrew nss formula). Real certutil wiring lands in a
        // follow-up.
        Ok(NssOutcome {
            profiles_attempted: 0,
            profiles_succeeded: 0,
            failures: vec![],
            certutil_missing: true,
        })
    }
}

/// macOS `ResolverInstaller` implementation.
#[derive(Debug, Default, Clone, Copy)]
pub struct MacosResolverInstaller;

impl MacosResolverInstaller {
    /// Construct.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl ResolverInstaller for MacosResolverInstaller {
    fn install(&self, tld: &str, _addr: SocketAddr) -> Result<(), PlatformError> {
        if tld.is_empty() {
            return Err(PlatformError::Resolver {
                reason: ResolverErrorReason::TldEmpty,
            });
        }
        Err(PlatformError::NeedsHelper {
            operation: ops::INSTALL_RESOLVER,
        })
    }

    fn uninstall(&self, tld: &str) -> Result<(), PlatformError> {
        if tld.is_empty() {
            return Err(PlatformError::Resolver {
                reason: ResolverErrorReason::TldEmpty,
            });
        }
        Err(PlatformError::NeedsHelper {
            operation: ops::UNINSTALL_RESOLVER,
        })
    }

    fn is_installed(&self, tld: &str, addr: SocketAddr) -> Result<bool, PlatformError> {
        if tld.is_empty() {
            return Err(PlatformError::Resolver {
                reason: ResolverErrorReason::TldEmpty,
            });
        }
        let path = resolver_file_path(tld);
        let Ok(text) = fs::read_to_string(&path) else {
            return Ok(false);
        };
        // Require the file to actually point at the daemon's DNS responder
        // (nameserver AND port). A bare `nameserver 127.0.0.1` left by Valet/Herd
        // or an older Yerd defaults to port 53 — where nothing listens — so it
        // must read as NOT installed and get rewritten with the right port.
        Ok(resolver_file::matches(&text, addr))
    }
}

fn resolver_file_path(tld: &str) -> PathBuf {
    PathBuf::from(format!("/etc/resolver/{tld}"))
}

/// macOS `PortBinder` implementation.
#[derive(Debug, Default, Clone, Copy)]
pub struct MacosPortBinder;

impl MacosPortBinder {
    /// Construct.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

fn bind_loopback(port: u16) -> std::io::Result<TcpListener> {
    TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))
}

impl PortBinder for MacosPortBinder {
    fn bind(&self, port: u16) -> Result<BoundPort, PlatformError> {
        bind_loopback(port)
            .map(|listener| BoundPort { listener })
            .map_err(|source| PlatformError::Bind { port, source })
    }

    fn bind_pair(
        &self,
        desired: (u16, u16),
        fallback: (u16, u16),
    ) -> Result<PortPair, PlatformError> {
        bind_pair_impl(desired, fallback)
    }
}

fn bind_pair_impl(desired: (u16, u16), fallback: (u16, u16)) -> Result<PortPair, PlatformError> {
    let http_attempt = bind_loopback(desired.0);
    let https_attempt = bind_loopback(desired.1);

    let http_outcome = http_attempt
        .as_ref()
        .map(|_| ())
        .map_err(std::io::Error::kind);
    let https_outcome = https_attempt
        .as_ref()
        .map(|_| ())
        .map_err(std::io::Error::kind);

    match port_plan::classify_desired(http_outcome, https_outcome) {
        port_plan::DesiredPairAction::KeepDesired => Ok(PortPair {
            http: BoundPort {
                listener: http_attempt.map_err(|e| PlatformError::Bind {
                    port: desired.0,
                    source: e,
                })?,
            },
            https: BoundPort {
                listener: https_attempt.map_err(|e| PlatformError::Bind {
                    port: desired.1,
                    source: e,
                })?,
            },
        }),
        port_plan::DesiredPairAction::HardFail(_) => {
            if let Err(e) = http_attempt {
                return Err(PlatformError::Bind {
                    port: desired.0,
                    source: e,
                });
            }
            if let Err(e) = https_attempt {
                return Err(PlatformError::Bind {
                    port: desired.1,
                    source: e,
                });
            }
            Err(PlatformError::Bind {
                port: desired.0,
                source: std::io::Error::from(std::io::ErrorKind::Other),
            })
        }
        port_plan::DesiredPairAction::UseFallback => {
            let desired_http_kind = http_outcome.err().unwrap_or(std::io::ErrorKind::Other);
            let desired_https_kind = https_outcome.err().unwrap_or(std::io::ErrorKind::Other);
            drop(http_attempt);
            drop(https_attempt);

            let fb_http = bind_loopback(fallback.0);
            let fb_https = bind_loopback(fallback.1);

            let fb_http_outcome = fb_http.as_ref().map(|_| ()).map_err(std::io::Error::kind);
            let fb_https_outcome = fb_https.as_ref().map(|_| ()).map_err(std::io::Error::kind);

            match port_plan::classify_fallback(fb_http_outcome, fb_https_outcome) {
                port_plan::FallbackPairAction::KeepFallback => Ok(PortPair {
                    http: BoundPort {
                        listener: fb_http.map_err(|e| PlatformError::Bind {
                            port: fallback.0,
                            source: e,
                        })?,
                    },
                    https: BoundPort {
                        listener: fb_https.map_err(|e| PlatformError::Bind {
                            port: fallback.1,
                            source: e,
                        })?,
                    },
                }),
                port_plan::FallbackPairAction::BothFailed => Err(PlatformError::BindPair {
                    reason: BindPairErrorReason::BothPairsFailed {
                        desired_http: desired_http_kind,
                        desired_https: desired_https_kind,
                        fallback_http: fb_http_outcome.err().unwrap_or(std::io::ErrorKind::Other),
                        fallback_https: fb_https_outcome.err().unwrap_or(std::io::ErrorKind::Other),
                    },
                }),
            }
        }
    }
}

/// macOS `SystemMetrics` implementation.
///
/// RSS is read from `ps -o rss= -p <pid>` (no `unsafe`-free per-process RSS
/// source exists in `std`), delegating the parse to
/// [`crate::pure::ps_metrics`]. Every failure collapses to `None` — metrics are
/// best-effort. `load_average` remains unimplemented (the Services UI shows
/// only memory; a `getloadavg`/`sysctl`-based impl can land later).
#[derive(Debug, Default, Clone, Copy)]
pub struct MacosSystemMetrics;

impl MacosSystemMetrics {
    /// Construct.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl SystemMetrics for MacosSystemMetrics {
    fn rss_bytes(&self, pid: u32) -> Option<u64> {
        // `-o rss=` prints headerless RSS in KiB; an absolute path keeps this
        // deterministic under the daemon's minimal PATH. A missing pid exits
        // non-zero with empty stdout, so both guards below collapse to `None`.
        let output = Command::new("/bin/ps")
            .args(["-o", "rss=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        ps_metrics::parse_ps_rss_bytes(&stdout)
    }

    fn load_average(&self) -> Option<[f64; 3]> {
        None
    }
}

/// macOS `PortRedirector` implementation.
#[derive(Debug, Default, Clone, Copy)]
pub struct MacosPortRedirector;

impl MacosPortRedirector {
    /// Construct.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl PortRedirector for MacosPortRedirector {
    fn is_active(&self) -> Option<bool> {
        // The pf redirect installs 80 and 443 together. Require the HTTP half to
        // actually reach *this* proxy (the `Server: yerd` marker) — not merely
        // that something answers on :80, which would false-green when a foreign
        // listener or a stale `pf` rule still holds the port after the user
        // removed the redirect. The HTTPS half only needs reachability: it's
        // installed by the same rule, and confirming it would need a TLS
        // handshake. So a yerd-confirmed :80 plus a reachable :443 means the
        // redirect is live and ours.
        Some(loopback_redirect_reaches_proxy(80) && loopback_port_reachable(443))
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn resolver_file_path_shape() {
        let p = resolver_file_path("test");
        assert_eq!(p, Path::new("/etc/resolver/test"));
    }
}
