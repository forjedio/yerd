//! Windows implementations of the platform traits.
//!
//! Paths, terminal launch, and TCP binding are available without elevation.
//! Trust-store and resolver integration remain explicit unsupported operations.

#![allow(clippy::similar_names)]

use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::Path;
use std::process::Command;

use directories::ProjectDirs;

use crate::error::ops;
use crate::metrics::SystemMetrics;
use crate::paths::{Paths, PlatformDirs};
use crate::port_binder::{BoundPort, PortBinder, PortPair};
use crate::port_redirect::PortRedirector;
use crate::pure::port_plan;
use crate::resolver::ResolverInstaller;
use crate::terminal::TerminalLauncher;
use crate::trust_store::{CaFingerprint, NssOutcome, TrustStore};
use crate::{BindPairErrorReason, PlatformError, TerminalErrorReason};

/// Windows terminal launcher.
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsTerminalLauncher;

impl WindowsTerminalLauncher {
    /// Construct.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl TerminalLauncher for WindowsTerminalLauncher {
    fn open_terminal(&self, path: &Path) -> Result<(), PlatformError> {
        if Command::new("wt.exe").arg("-d").arg(path).spawn().is_ok() {
            return Ok(());
        }
        Command::new("cmd.exe")
            .arg("/K")
            .current_dir(path)
            .spawn()
            .map(|_| ())
            .map_err(|_| PlatformError::Terminal {
                reason: TerminalErrorReason::NoSupportedTerminal,
            })
    }
}

/// Windows path implementation.
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
        let pd = ProjectDirs::from("io", "yerd", "Yerd").ok_or(PlatformError::MissingHomeDir)?;
        let data = pd.data_dir().to_path_buf();
        let local = pd.data_local_dir().to_path_buf();
        Ok(PlatformDirs {
            config: pd.config_dir().to_path_buf(),
            data,
            state: local.clone(),
            cache: pd.cache_dir().to_path_buf(),
            runtime: local.join("run"),
        })
    }
}

/// Windows trust-store stub until privileged certificate integration is available.
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsTrustStore;

impl WindowsTrustStore {
    /// Construct.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl TrustStore for WindowsTrustStore {
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

    fn is_present_system(&self, _: &CaFingerprint) -> Result<bool, PlatformError> {
        Err(PlatformError::Unsupported {
            operation: ops::IS_PRESENT_SYSTEM,
        })
    }

    fn is_trusted(&self, ca_path: &Path, fp: &CaFingerprint) -> Result<bool, PlatformError> {
        let pem = std::fs::read_to_string(ca_path).map_err(|source| PlatformError::Io {
            path: ca_path.to_path_buf(),
            source,
        })?;
        if CaFingerprint::from_pem(&pem).as_ref() != Some(fp) {
            return Ok(false);
        }
        let output = Command::new(r"C:\Windows\System32\certutil.exe")
            .args(["-verify"])
            .arg(ca_path)
            .output()
            .map_err(|source| PlatformError::Io {
                path: ca_path.to_path_buf(),
                source,
            })?;
        Ok(output.status.success())
    }

    fn install_firefox_nss(&self, _: &Path) -> Result<NssOutcome, PlatformError> {
        Err(PlatformError::Unsupported {
            operation: ops::INSTALL_FIREFOX_NSS,
        })
    }

    fn uninstall_firefox_nss(&self) -> Result<NssOutcome, PlatformError> {
        Err(PlatformError::Unsupported {
            operation: ops::UNINSTALL_FIREFOX_NSS,
        })
    }

    fn system_root_bundle(&self) -> Result<Option<String>, PlatformError> {
        Ok(None)
    }
}

/// Windows resolver stub until NRPT or DNS policy integration is available.
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsResolverInstaller;

impl WindowsResolverInstaller {
    /// Construct.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl ResolverInstaller for WindowsResolverInstaller {
    fn install(&self, _: &str, _: SocketAddr) -> Result<(), PlatformError> {
        Err(PlatformError::NeedsHelper {
            operation: ops::INSTALL_RESOLVER,
        })
    }

    fn uninstall(&self, _: &str) -> Result<(), PlatformError> {
        Err(PlatformError::NeedsHelper {
            operation: ops::UNINSTALL_RESOLVER,
        })
    }

    fn is_installed(&self, tld: &str, addr: SocketAddr) -> Result<bool, PlatformError> {
        if tld.is_empty() {
            return Err(PlatformError::Resolver {
                reason: crate::ResolverErrorReason::TldEmpty,
            });
        }
        if addr != SocketAddr::from((Ipv4Addr::LOCALHOST, 53)) {
            return Ok(false);
        }
        let tld = yerd_core::Tld::new(tld).map_err(|error| PlatformError::Resolver {
            reason: crate::ResolverErrorReason::SystemApi(error.to_string()),
        })?;
        let command = format!(
            "if (Get-DnsClientNrptRule | Where-Object {{ $_.Namespace -contains '.{}' -and $_.NameServers -contains '127.0.0.1' -and $_.Comment -eq 'Yerd managed' }}) {{ exit 0 }} else {{ exit 1 }}",
            tld.as_str()
        );
        let status = Command::new(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", &command])
            .status()
            .map_err(|source| PlatformError::Resolver {
                reason: crate::ResolverErrorReason::SystemApi(source.to_string()),
            })?;
        Ok(status.success())
    }
}

/// Windows TCP port binder.
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
                listener: http_attempt.map_err(|source| PlatformError::Bind {
                    port: desired.0,
                    source,
                })?,
            },
            https: BoundPort {
                listener: https_attempt.map_err(|source| PlatformError::Bind {
                    port: desired.1,
                    source,
                })?,
            },
        }),
        port_plan::DesiredPairAction::HardFail(_) => {
            if let Err(source) = http_attempt {
                return Err(PlatformError::Bind {
                    port: desired.0,
                    source,
                });
            }
            if let Err(source) = https_attempt {
                return Err(PlatformError::Bind {
                    port: desired.1,
                    source,
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
            let fallback_http = bind_at(ip, fallback.0);
            let fallback_https = bind_at(ip, fallback.1);
            let fallback_http_outcome = fallback_http
                .as_ref()
                .map(|_| ())
                .map_err(std::io::Error::kind);
            let fallback_https_outcome = fallback_https
                .as_ref()
                .map(|_| ())
                .map_err(std::io::Error::kind);

            match port_plan::classify_fallback(fallback_http_outcome, fallback_https_outcome) {
                port_plan::FallbackPairAction::KeepFallback => Ok(PortPair {
                    http: BoundPort {
                        listener: fallback_http.map_err(|source| PlatformError::Bind {
                            port: fallback.0,
                            source,
                        })?,
                    },
                    https: BoundPort {
                        listener: fallback_https.map_err(|source| PlatformError::Bind {
                            port: fallback.1,
                            source,
                        })?,
                    },
                }),
                port_plan::FallbackPairAction::BothFailed => Err(PlatformError::BindPair {
                    reason: BindPairErrorReason::BothPairsFailed {
                        desired_http: desired_http_kind,
                        desired_https: desired_https_kind,
                        fallback_http: fallback_http_outcome
                            .err()
                            .unwrap_or(std::io::ErrorKind::Other),
                        fallback_https: fallback_https_outcome
                            .err()
                            .unwrap_or(std::io::ErrorKind::Other),
                    },
                }),
            }
        }
    }
}

/// Best-effort Windows metrics placeholder.
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsSystemMetrics;

impl WindowsSystemMetrics {
    /// Construct.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl SystemMetrics for WindowsSystemMetrics {
    fn rss_bytes(&self, _: u32) -> Option<u64> {
        None
    }

    fn load_average(&self) -> Option<[f64; 3]> {
        None
    }
}

/// Windows port redirect probe placeholder.
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsPortRedirector;

impl WindowsPortRedirector {
    /// Construct.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl PortRedirector for WindowsPortRedirector {
    fn is_active(&self) -> Option<bool> {
        None
    }
}
