//! Windows OS implementation.
//!
//! Only [`WindowsPaths`] is a real implementation in Phase 1. Every other trait
//! is a type alias to the `os::unsupported` stub, so the trait impls come for
//! free and stay total. Later phases replace one alias at a time with a real
//! `Windows*` type in the same change that adds its full trait impl (the
//! "never half-flip" rule).

#![allow(clippy::similar_names)]

use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::OnceLock;

use crate::paths::{Paths, PlatformDirs};
use crate::port_binder::{BoundPort, PortBinder, PortPair};
use crate::pure::{port_plan, win_pipe};
use crate::{BindPairErrorReason, PlatformError};

pub use super::unsupported::{
    UnsupportedPortRedirector as WindowsPortRedirector,
    UnsupportedResolverInstaller as WindowsResolverInstaller,
    UnsupportedSystemMetrics as WindowsSystemMetrics,
    UnsupportedTerminalLauncher as WindowsTerminalLauncher,
    UnsupportedTrustStore as WindowsTrustStore,
};

/// Read `%VAR%` as a non-empty directory path, or `MissingHomeDir` when unset or
/// empty. Windows has no single `HOME`; the known-folder env vars are the
/// closest equivalent, so a missing one reuses that error rather than adding a
/// near-duplicate variant.
fn env_dir(var: &str) -> Result<PathBuf, PlatformError> {
    match std::env::var_os(var) {
        Some(v) if !v.is_empty() => Ok(PathBuf::from(v)),
        _ => Err(PlatformError::MissingHomeDir),
    }
}

/// Real `Paths` for Windows.
///
/// Reads the known-folder env vars directly rather than using the `directories`
/// crate, whose Windows mapping (`%APPDATA%\Yerd\config`, different casing and
/// nesting) does not match Yerd's locked layout.
///
/// Layout decisions:
/// - `config` = `%APPDATA%\yerd` (roaming, like the Unix config home).
/// - `data`/`state`/`cache` are subdirectories of one `%LOCALAPPDATA%\yerd`
///   root so an uninstall can remove a single tree plus `%APPDATA%\yerd`.
/// - `state` stays distinct from `data` (as on Linux, unlike macOS): cheap now,
///   avoids a migration later.
/// - `runtime` = `std::env::temp_dir().join("yerd")`. It is per-user because
///   `%TEMP%` is per-user on Windows, so there is no `/tmp` sticky-bit trade-off
///   as on Linux.
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsPaths;

impl WindowsPaths {
    /// Construct.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Paths for WindowsPaths {
    fn resolve(&self) -> Result<PlatformDirs, PlatformError> {
        let config = env_dir("APPDATA")?.join("yerd");
        let local = env_dir("LOCALAPPDATA")?.join("yerd");
        Ok(PlatformDirs {
            config,
            data: local.join("data"),
            state: local.join("state"),
            cache: local.join("cache"),
            runtime: std::env::temp_dir().join("yerd"),
        })
    }
}

/// Windows `PortBinder` implementation.
///
/// Sub-1024 binds are unprivileged on Windows, so unlike Linux/macOS there is no
/// `setcap`/`pf` special-casing: `bind_pair` uses the same generic desired →
/// fallback retry as Linux, attempting the desired ports directly. Pulled forward
/// from Phase 3 (Phase 2's FPM pool needs an ephemeral loopback bind; Phase 3
/// adds the 80/443 conflict validation and doctor check on top).
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsPortBinder;

impl WindowsPortBinder {
    /// Construct.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

fn bind_at(ip: Ipv4Addr, port: u16) -> std::io::Result<TcpListener> {
    TcpListener::bind(SocketAddr::from((ip, port)))
}

impl PortBinder for WindowsPortBinder {
    fn bind(&self, port: u16) -> Result<BoundPort, PlatformError> {
        bind_at(Ipv4Addr::LOCALHOST, port)
            .map(|listener| BoundPort { listener })
            .map_err(|source| PlatformError::Bind { port, source })
    }

    fn bind_pair(
        &self,
        lan: bool,
        desired: (u16, u16),
        fallback: (u16, u16),
    ) -> Result<PortPair, PlatformError> {
        bind_pair_impl(lan, desired, fallback)
    }
}

/// The generic desired → fallback bind-pair retry (Linux shape, no privilege
/// special-casing). Attempt `desired`; on a retry-trigger kind
/// (`PermissionDenied`/`AddrInUse`/`AddrNotAvailable`) drop any partial listener
/// and retry `fallback`; any other error on the desired pair surfaces
/// immediately; if both pairs fail, a [`PlatformError::BindPair`] carries all
/// four `ErrorKind`s.
fn bind_pair_impl(
    lan: bool,
    desired: (u16, u16),
    fallback: (u16, u16),
) -> Result<PortPair, PlatformError> {
    let ip = if lan {
        Ipv4Addr::UNSPECIFIED
    } else {
        Ipv4Addr::LOCALHOST
    };
    let http_attempt = bind_at(ip, desired.0);
    let https_attempt = bind_at(ip, desired.1);

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

            let fb_http = bind_at(ip, fallback.0);
            let fb_https = bind_at(ip, fallback.1);

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

/// Process-lifetime cache of the resolved SID, so [`current_user_sid`] spawns
/// `whoami` at most once per process.
static USER_SID: OnceLock<String> = OnceLock::new();

/// The current user's SID, e.g. `S-1-5-21-...`.
///
/// Runs `%SystemRoot%\System32\whoami.exe /user /fo csv /nh` (an absolute path,
/// never trusting `PATH`) and parses the SID from its CSV output. Cached for the
/// process lifetime. No `unsafe` and no new crates: the `GetTokenInformation`
/// alternative is `unsafe` FFI, which this crate's `#![forbid(unsafe_code)]`
/// rules out.
pub fn current_user_sid() -> Result<String, PlatformError> {
    if let Some(sid) = USER_SID.get() {
        return Ok(sid.clone());
    }
    let sid = spawn_whoami_sid()?;
    Ok(USER_SID.get_or_init(|| sid).clone())
}

/// Absolute path to `whoami.exe`, from `%SystemRoot%` (falling back to the
/// conventional location), so the lookup never resolves an attacker-planted
/// `whoami` on `PATH`.
fn whoami_path() -> PathBuf {
    let root =
        std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    root.join("System32").join("whoami.exe")
}

fn spawn_whoami_sid() -> Result<String, PlatformError> {
    let output = std::process::Command::new(whoami_path())
        .args(["/user", "/fo", "csv", "/nh"])
        .output()
        .map_err(|e| PlatformError::SidLookup {
            detail: format!("whoami spawn failed: {e}"),
        })?;
    if !output.status.success() {
        return Err(PlatformError::SidLookup {
            detail: format!("whoami exited with {}", output.status),
        });
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    win_pipe::parse_whoami_sid(&stdout).ok_or_else(|| PlatformError::SidLookup {
        detail: "whoami output had no parseable SID".to_owned(),
    })
}

/// The daemon pipe name for the current user under `dirs.runtime`: the single
/// shared derivation used by the daemon listener and every client.
pub fn daemon_pipe_name(dirs: &PlatformDirs) -> Result<String, PlatformError> {
    Ok(win_pipe::pipe_name(&current_user_sid()?, &dirs.runtime))
}
