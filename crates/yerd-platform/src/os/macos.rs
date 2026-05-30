//! macOS implementations of the four traits.
//!
//! `Paths` uses `directories` for `config`/`data`/`cache`; `state`
//! coincides with `data` on macOS (no XDG state distinction); `runtime`
//! is a `yerd-$UID` directory inside `std::env::temp_dir()`.
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

use directories::ProjectDirs;

use crate::error::ops;
use crate::metrics::SystemMetrics;
use crate::paths::{Paths, PlatformDirs};
use crate::port_binder::{BoundPort, PortBinder, PortPair};
use crate::pure::{pem_match, port_plan, resolver_file};
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

        // No XDG_RUNTIME_DIR on macOS — use a per-user dir inside the
        // standard temp dir. Caller should still set mode 0o700.
        let uid = read_real_uid().unwrap_or(0);
        let runtime = std::env::temp_dir().join(format!("yerd-{uid}"));

        Ok(PlatformDirs {
            config,
            data,
            state,
            cache,
            runtime,
        })
    }
}

/// Read the real UID via the `id -u` command, which is available on
/// every macOS install. `std::process::Command` is acceptable here
/// because (a) the input is constant, (b) the output is parsed as a
/// `u32`, (c) no privilege boundary is crossed.
fn read_real_uid() -> Option<u32> {
    let out = std::process::Command::new("id").arg("-u").output().ok()?;
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

    fn is_installed(&self, tld: &str) -> Result<bool, PlatformError> {
        if tld.is_empty() {
            return Err(PlatformError::Resolver {
                reason: ResolverErrorReason::TldEmpty,
            });
        }
        let path = resolver_file_path(tld);
        let Ok(text) = fs::read_to_string(&path) else {
            return Ok(false);
        };
        // Parse-only structural check; we accept any well-formed
        // /etc/resolver/<tld> as evidence the redirect is in place.
        Ok(resolver_file::parse(&text).is_some())
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
/// Phase 1 has no cheap, `unsafe`-free RSS/load source on macOS, so both
/// methods return `None` (best-effort: callers show nothing). A `sysctl`/
/// `proc_pid_rusage`-based impl can land post-MVP.
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
    fn rss_bytes(&self, _: u32) -> Option<u64> {
        None
    }

    fn load_average(&self) -> Option<[f64; 3]> {
        None
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
    use super::*;

    #[test]
    fn resolver_file_path_shape() {
        let p = resolver_file_path("test");
        assert_eq!(p, Path::new("/etc/resolver/test"));
    }
}
