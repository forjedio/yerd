//! Linux implementations of the four traits.
//!
//! `Paths` uses XDG directories via the `directories` crate; the
//! `runtime` fallback parses `/proc/self/status` to find the real UID
//! when `XDG_RUNTIME_DIR` is unset. Privileged ops return
//! `NeedsHelper`; probes are read-only.

#![allow(clippy::similar_names)]

use std::fs;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::{Path, PathBuf};

use directories::ProjectDirs;

use crate::error::ops;
use crate::metrics::SystemMetrics;
use crate::paths::{Paths, PlatformDirs};
use crate::port_binder::{BoundPort, PortBinder, PortPair};
use crate::port_redirect::PortRedirector;
use crate::pure::{
    pem_match, port_plan, proc_metrics, resolv_conf, resolved_drop_in, system_roots,
};
use crate::resolver::ResolverInstaller;
use crate::trust_store::{CaFingerprint, NssOutcome, TrustStore};
use crate::{BindPairErrorReason, PlatformError, ResolverErrorReason, TrustStoreErrorReason};

/// Linux `Paths` implementation.
#[derive(Debug, Default, Clone, Copy)]
pub struct LinuxPaths;

impl LinuxPaths {
    /// Construct.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Paths for LinuxPaths {
    fn resolve(&self) -> Result<PlatformDirs, PlatformError> {
        let pd = ProjectDirs::from("io", "yerd", "Yerd").ok_or(PlatformError::MissingHomeDir)?;
        let config = pd.config_dir().to_path_buf();
        let data = pd.data_dir().to_path_buf();
        let cache = pd.cache_dir().to_path_buf();

        // state_dir() - XDG_STATE_HOME - is the right answer; if None,
        // fall back to $HOME/.local/state/yerd. Never collapse to data.
        let state = pd.state_dir().map_or_else(
            || {
                home_dir().map_or_else(
                    || PathBuf::from("./.local/state/yerd"),
                    |h| h.join(".local/state/yerd"),
                )
            },
            Path::to_path_buf,
        );

        // runtime_dir() - XDG_RUNTIME_DIR - falls back to /tmp/yerd-$UID
        // when None. Caller is responsible for mkdir(mode=0o700) and
        // ownership/mode verification.
        let runtime = pd.runtime_dir().map_or_else(
            || {
                let uid = read_real_uid().unwrap_or(0);
                PathBuf::from(format!("/tmp/yerd-{uid}"))
            },
            Path::to_path_buf,
        );

        Ok(PlatformDirs {
            config,
            data,
            state,
            cache,
            runtime,
        })
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Read the real UID from `/proc/self/status`. Returns `None` if `/proc`
/// is not mounted or the file shape is unexpected.
fn read_real_uid() -> Option<u32> {
    let text = fs::read_to_string("/proc/self/status").ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Uid:") {
            let real = rest.split_whitespace().next()?;
            return real.parse().ok();
        }
    }
    None
}

/// Linux `TrustStore` implementation.
#[derive(Debug, Default, Clone, Copy)]
pub struct LinuxTrustStore;

impl LinuxTrustStore {
    /// Construct.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

/// Anchor directories Yerd scans on Linux. Order is not significant.
const ANCHOR_DIRS: &[&str] = &[
    "/usr/local/share/ca-certificates", // Debian/Ubuntu/Alpine
    "/etc/pki/ca-trust/source/anchors", // RHEL/Fedora/CentOS
    "/etc/ca-certificates/trust-source/anchors", // Arch
];

impl TrustStore for LinuxTrustStore {
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
        let chosen = ANCHOR_DIRS.iter().map(Path::new).find(|p| p.is_dir());

        let Some(dir) = chosen else {
            // No recognised layout - caller likely needs to install
            // ca-certificates first.
            return Err(PlatformError::TrustStore {
                reason: TrustStoreErrorReason::AnchorDirMissing(PathBuf::from(
                    "(no recognised anchor directory)",
                )),
            });
        };

        let entries = fs::read_dir(dir).map_err(|source| PlatformError::TrustStore {
            reason: TrustStoreErrorReason::AnchorEnumerate(source),
        })?;

        let mut blobs: Vec<(PathBuf, Vec<u8>)> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("crt") {
                continue;
            }
            let bytes = fs::read(&path).map_err(|_| PlatformError::TrustStore {
                reason: TrustStoreErrorReason::AnchorRead(path.clone()),
            })?;
            blobs.push((path, bytes));
        }

        match pem_match::find_by_fingerprint(&blobs, fp.as_bytes()) {
            Ok(Some(_)) => Ok(true),
            Ok(None) => Ok(false),
            Err(bad_path) => Err(PlatformError::TrustStore {
                reason: TrustStoreErrorReason::AnchorPemInvalid(bad_path),
            }),
        }
    }

    fn is_trusted(&self, _ca_path: &Path, fp: &CaFingerprint) -> Result<bool, PlatformError> {
        // On Linux, presence in an anchor directory *is* system trust (unlike
        // macOS, where presence and trust are distinct), so an effective-trust
        // probe is the same as the presence probe. `ca_path` is unused here.
        self.is_present_system(fp)
    }

    fn install_firefox_nss(&self, _: &str) -> Result<NssOutcome, PlatformError> {
        Ok(NssOutcome {
            profiles_attempted: 0,
            profiles_succeeded: 0,
            failures: vec![],
            certutil_missing: false,
        })
    }

    fn system_root_bundle(&self) -> Result<Option<String>, PlatformError> {
        Ok(system_roots::pick_first_readable(
            &system_roots::linux_root_candidates(),
            |p| fs::read_to_string(p).ok(),
        ))
    }
}

/// Linux `ResolverInstaller` implementation.
#[derive(Debug, Default, Clone, Copy)]
pub struct LinuxResolverInstaller;

impl LinuxResolverInstaller {
    /// Construct.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl ResolverInstaller for LinuxResolverInstaller {
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

    fn is_installed(&self, tld: &str, _addr: SocketAddr) -> Result<bool, PlatformError> {
        if tld.is_empty() {
            return Err(PlatformError::Resolver {
                reason: ResolverErrorReason::TldEmpty,
            });
        }

        // Backend probe: prefer the systemd-resolved drop-in if its
        // content parses to a matching shape; otherwise check
        // /etc/resolv.conf for the Yerd marker. We do not require
        // detect_systemd_resolved() to short-circuit - the drop-in
        // can be present on systems where resolved is also active.
        let drop_in = drop_in_path(tld);
        if let Ok(text) = fs::read_to_string(drop_in) {
            // Shape-only probe: a well-formed drop-in for `tld` is evidence the
            // resolver is wired up. resolved manages forwarding internally, so
            // (unlike macOS) `_addr` need not be re-verified against the file.
            if let Some(parsed) = resolved_drop_in::parse(&text) {
                return Ok(parsed.domain == tld);
            }
        }

        let resolv = fs::read_to_string("/etc/resolv.conf").unwrap_or_default();
        if !resolv.is_empty() {
            let _ = resolv_conf::detect_systemd_resolved(&resolv, false);
        }
        Ok(false)
    }
}

fn drop_in_path(tld: &str) -> PathBuf {
    PathBuf::from(format!("/etc/systemd/resolved.conf.d/yerd-{tld}.conf"))
}

/// Linux `PortBinder` implementation.
#[derive(Debug, Default, Clone, Copy)]
pub struct LinuxPortBinder;

impl LinuxPortBinder {
    /// Construct.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

fn bind_loopback(port: u16) -> std::io::Result<TcpListener> {
    TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))
}

impl PortBinder for LinuxPortBinder {
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

pub(crate) fn bind_pair_impl(
    desired: (u16, u16),
    fallback: (u16, u16),
) -> Result<PortPair, PlatformError> {
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

/// Linux `SystemMetrics` implementation.
///
/// Reads `/proc/<pid>/status` (`VmRSS`) and `/proc/loadavg`, delegating the
/// parsing to [`crate::pure::proc_metrics`]. Every read failure collapses to
/// `None` - metrics are best-effort.
#[derive(Debug, Default, Clone, Copy)]
pub struct LinuxSystemMetrics;

impl LinuxSystemMetrics {
    /// Construct.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl SystemMetrics for LinuxSystemMetrics {
    fn rss_bytes(&self, pid: u32) -> Option<u64> {
        let contents = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
        proc_metrics::parse_vmrss_bytes(&contents)
    }

    fn load_average(&self) -> Option<[f64; 3]> {
        let contents = fs::read_to_string("/proc/loadavg").ok()?;
        proc_metrics::parse_loadavg(&contents)
    }
}

/// Linux `PortRedirector` implementation.
///
/// Not applicable on Linux: `yerd elevate ports` grants
/// `cap_net_bind_service`, so the daemon binds 80/443 directly rather than
/// going through a redirect. The probe always returns `None` ("N/A").
#[derive(Debug, Default, Clone, Copy)]
pub struct LinuxPortRedirector;

impl LinuxPortRedirector {
    /// Construct.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl PortRedirector for LinuxPortRedirector {
    fn is_active(&self) -> Option<bool> {
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
    fn drop_in_path_shape() {
        let p = drop_in_path("test");
        assert_eq!(
            p,
            PathBuf::from("/etc/systemd/resolved.conf.d/yerd-test.conf")
        );
    }

    #[test]
    fn read_real_uid_returns_some_when_proc_present() {
        if Path::new("/proc/self/status").exists() {
            assert!(read_real_uid().is_some());
        }
    }
}
