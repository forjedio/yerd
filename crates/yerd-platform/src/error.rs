//! Error types for `yerd-platform`.
//!
//! [`PlatformError`] is the single error type exposed by every fallible
//! public API in this crate. Each variant that needs more detail carries a
//! typed `*Reason` sub-enum so callers can match on precise failure modes
//! without parsing message strings.
//!
//! Unlike `yerd-tls`, `PlatformError` is **not** `Clone + Eq` because it
//! wraps [`std::io::Error`] in two variants - the same pattern as
//! `yerd-config::ConfigError`. Each reason sub-enum is `#[non_exhaustive]`
//! so additions are semver-compatible.

use std::path::PathBuf;

use thiserror::Error;

/// Single error type exposed by every fallible public API in this crate.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum PlatformError {
    /// The requested operation must be performed by `yerd-helper`. The
    /// daemon is responsible for materialising the matching
    /// [`crate::HelperInvocation`] and invoking the helper.
    #[error("operation requires helper invocation: {operation}")]
    NeedsHelper {
        /// Operation tag from [`ops`].
        operation: &'static str,
    },

    /// The current OS has no implementation of this operation.
    #[error("operation not supported on this OS: {operation}")]
    Unsupported {
        /// Operation tag from [`ops`].
        operation: &'static str,
    },

    /// `$HOME` (or its OS equivalent) could not be resolved.
    #[error("HOME directory could not be resolved")]
    MissingHomeDir,

    /// Trust-store failure with typed reason.
    #[error("trust store: {reason}")]
    TrustStore {
        /// Specific failure.
        reason: TrustStoreErrorReason,
    },

    /// Resolver-installation failure with typed reason.
    #[error("resolver: {reason}")]
    Resolver {
        /// Specific failure.
        reason: ResolverErrorReason,
    },

    /// A single-port `bind` call failed.
    #[error("port {port} could not be bound: {source}")]
    Bind {
        /// The port the caller asked for.
        port: u16,
        /// Underlying OS error.
        #[source]
        source: std::io::Error,
    },

    /// Both the desired and fallback port pairs failed.
    #[error("port-pair bind failed: {reason:?}")]
    BindPair {
        /// Specific failure mode.
        reason: BindPairErrorReason,
    },

    /// The host's own LAN IPv4 could not be determined (see
    /// [`crate::LanIpProvider`]). LAN mode fails closed when this happens.
    #[error("LAN IP discovery failed: {source}")]
    LanIpDiscovery {
        /// Underlying OS error from the discovery probe.
        #[source]
        source: std::io::Error,
    },

    /// Generic I/O error against a known path.
    #[error("I/O at {path}: {source}", path = path.display())]
    Io {
        /// The path the I/O was directed at.
        path: PathBuf,
        /// Underlying OS error.
        #[source]
        source: std::io::Error,
    },

    /// A required external command is not on `PATH`.
    #[error("required external command missing: {tool}{}", display_install_hint(*install_hint))]
    MissingTool {
        /// Binary basename that was looked up (e.g. `"certutil"`).
        tool: &'static str,
        /// Optional human-readable install hint (e.g. `"install nss"`).
        install_hint: Option<&'static str>,
    },
}

fn display_install_hint(hint: Option<&'static str>) -> String {
    match hint {
        Some(h) => format!(" (install via: {h})"),
        None => String::new(),
    }
}

/// Specific failure modes for [`PlatformError::TrustStore`].
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum TrustStoreErrorReason {
    /// Configured anchor directory is missing from the filesystem.
    #[error("anchor directory missing: {}", .0.display())]
    AnchorDirMissing(PathBuf),

    /// Failed to enumerate the anchor directory contents.
    #[error("could not enumerate anchor dir: {0}")]
    AnchorEnumerate(#[source] std::io::Error),

    /// Failed to read a single anchor file.
    #[error("could not read anchor file: {}", .0.display())]
    AnchorRead(PathBuf),

    /// Anchor file content was not valid PEM.
    #[error("PEM parse failed in anchor file: {}", .0.display())]
    AnchorPemInvalid(PathBuf),

    /// System trust API returned an error (e.g. `security-framework` call).
    #[error("system trust API failed: {0}")]
    SystemApi(String),

    /// `certutil` invocation completed with non-zero exit.
    #[error("NSS certutil failed (exit {0})")]
    NssCertutilFailed(i32),
}

/// Specific failure modes for [`PlatformError::Resolver`].
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ResolverErrorReason {
    /// TLD argument was empty.
    #[error("tld empty")]
    TldEmpty,

    /// `systemd-resolved` was expected but is not active.
    #[error("systemd-resolved not active")]
    ResolvedNotActive,

    /// `/etc/resolv.conf` could not be read.
    #[error("could not read resolv.conf")]
    ResolvConfUnreadable,

    /// Drop-in path could not be written (typically permission denied).
    #[error("drop-in path not writable: {}", .0.display())]
    DropInNotWritable(PathBuf),
}

/// Specific failure modes for [`PlatformError::BindPair`].
#[derive(Debug)]
#[non_exhaustive]
pub enum BindPairErrorReason {
    /// Both the desired pair and the fallback pair failed. All four
    /// `ErrorKind`s are preserved so the daemon can distinguish "setcap
    /// missing" (`PermissionDenied` across the board) from "port already
    /// in use" (`AddrInUse` on the desired pair) and message the user
    /// accordingly.
    BothPairsFailed {
        /// `ErrorKind` from the desired-pair HTTP bind attempt.
        desired_http: std::io::ErrorKind,
        /// `ErrorKind` from the desired-pair HTTPS bind attempt.
        desired_https: std::io::ErrorKind,
        /// `ErrorKind` from the fallback-pair HTTP bind attempt.
        fallback_http: std::io::ErrorKind,
        /// `ErrorKind` from the fallback-pair HTTPS bind attempt.
        fallback_https: std::io::ErrorKind,
    },
}

/// Operation tag constants. Single source of truth for the strings that
/// appear in [`PlatformError::NeedsHelper`], [`PlatformError::Unsupported`],
/// and the argv leading element produced by [`crate::HelperInvocation`].
pub mod ops {
    /// `Paths::resolve` on an unsupported OS.
    pub const PATHS_RESOLVE: &str = "paths-resolve";
    /// System trust-store install.
    pub const INSTALL_CA: &str = "install-ca";
    /// System trust-store uninstall.
    pub const UNINSTALL_CA: &str = "uninstall-ca";
    /// Per-user browser (Chromium/Firefox) NSS trust-store install.
    pub const INSTALL_FIREFOX_NSS: &str = "install-firefox-nss";
    /// Per-user browser (Chromium/Firefox) NSS trust-store uninstall.
    pub const UNINSTALL_FIREFOX_NSS: &str = "uninstall-firefox-nss";
    /// Per-user browser NSS effective-trust probe.
    pub const BROWSER_CA_TRUST: &str = "browser-ca-trust";
    /// System trust-store presence probe.
    pub const IS_PRESENT_SYSTEM: &str = "is-present-system";
    /// Effective-trust probe (trusted, not merely present).
    pub const IS_TRUSTED: &str = "is-trusted";
    /// Resolver install.
    pub const INSTALL_RESOLVER: &str = "install-resolver";
    /// Resolver uninstall.
    pub const UNINSTALL_RESOLVER: &str = "uninstall-resolver";
    /// Resolver-presence probe.
    pub const IS_INSTALLED_RESOLVER: &str = "is-installed-resolver";
    /// TCP listener bind.
    pub const BIND: &str = "bind";
    /// Atomic 80+443 / 8080+8443 bind.
    pub const BIND_PAIR: &str = "bind-pair";
    /// Apply `cap_net_bind_service` to the daemon binary.
    pub const SETCAP: &str = "setcap";
    /// Install the macOS pf redirect (80/443 → rootless ports).
    pub const INSTALL_PORT_REDIRECT: &str = "install-port-redirect";
    /// Remove the macOS pf redirect.
    pub const UNINSTALL_PORT_REDIRECT: &str = "uninstall-port-redirect";
    /// Install the macOS **LAN** pf redirect (80/443 → rootless on the LAN IP).
    pub const INSTALL_LAN_PORT_REDIRECT: &str = "install-lan-port-redirect";
    /// Remove the macOS LAN pf redirect.
    pub const UNINSTALL_LAN_PORT_REDIRECT: &str = "uninstall-lan-port-redirect";
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
    fn display_needs_helper_contains_operation() {
        let e = PlatformError::NeedsHelper {
            operation: ops::INSTALL_CA,
        };
        assert!(e.to_string().contains("install-ca"));
    }

    #[test]
    fn display_unsupported_contains_operation() {
        let e = PlatformError::Unsupported {
            operation: ops::PATHS_RESOLVE,
        };
        assert!(e.to_string().contains("paths-resolve"));
    }

    #[test]
    fn display_missing_home_dir() {
        assert!(PlatformError::MissingHomeDir.to_string().contains("HOME"));
    }

    #[test]
    fn display_bind_carries_port_and_source() {
        let e = PlatformError::Bind {
            port: 443,
            source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        };
        let s = e.to_string();
        assert!(s.contains("443"));
        assert!(s.contains("denied"));
    }

    #[test]
    fn display_io_carries_path() {
        let e = PlatformError::Io {
            path: PathBuf::from("/tmp/foo"),
            source: std::io::Error::from(std::io::ErrorKind::NotFound),
        };
        assert!(e.to_string().contains("/tmp/foo"));
    }

    #[test]
    fn display_missing_tool_with_hint() {
        let e = PlatformError::MissingTool {
            tool: "certutil",
            install_hint: Some("install nss"),
        };
        let s = e.to_string();
        assert!(s.contains("certutil"));
        assert!(s.contains("install nss"));
    }

    #[test]
    fn display_missing_tool_without_hint() {
        let e = PlatformError::MissingTool {
            tool: "certutil",
            install_hint: None,
        };
        let s = e.to_string();
        assert!(s.contains("certutil"));
        assert!(!s.contains("install via"));
    }

    #[test]
    fn display_trust_store_carries_reason() {
        let e = PlatformError::TrustStore {
            reason: TrustStoreErrorReason::NssCertutilFailed(2),
        };
        assert!(e.to_string().contains("exit 2"));
    }

    #[test]
    fn display_resolver_carries_reason() {
        let e = PlatformError::Resolver {
            reason: ResolverErrorReason::TldEmpty,
        };
        assert!(e.to_string().contains("tld"));
    }

    #[test]
    fn display_bind_pair_includes_all_four_kinds() {
        let e = PlatformError::BindPair {
            reason: BindPairErrorReason::BothPairsFailed {
                desired_http: std::io::ErrorKind::PermissionDenied,
                desired_https: std::io::ErrorKind::PermissionDenied,
                fallback_http: std::io::ErrorKind::AddrInUse,
                fallback_https: std::io::ErrorKind::AddrInUse,
            },
        };
        let s = e.to_string();
        assert!(s.contains("PermissionDenied"));
        assert!(s.contains("AddrInUse"));
    }

    #[test]
    fn trust_store_reason_anchor_dir_missing() {
        let r = TrustStoreErrorReason::AnchorDirMissing(PathBuf::from("/etc/pki/ca-trust"));
        assert!(r.to_string().contains("/etc/pki/ca-trust"));
    }

    #[test]
    fn trust_store_reason_anchor_enumerate() {
        let r = TrustStoreErrorReason::AnchorEnumerate(std::io::Error::from(
            std::io::ErrorKind::PermissionDenied,
        ));
        assert!(r.to_string().contains("enumerate"));
    }

    #[test]
    fn trust_store_reason_anchor_read_and_pem() {
        let p = PathBuf::from("/etc/ca/anchor.crt");
        let r1 = TrustStoreErrorReason::AnchorRead(p.clone());
        let r2 = TrustStoreErrorReason::AnchorPemInvalid(p.clone());
        assert!(r1.to_string().contains("anchor.crt"));
        assert!(r2.to_string().contains("anchor.crt"));
    }

    #[test]
    fn trust_store_reason_system_api() {
        let r = TrustStoreErrorReason::SystemApi("kSecTrustErr".to_string());
        assert!(r.to_string().contains("kSecTrustErr"));
    }

    #[test]
    fn resolver_reason_all_variants_display() {
        for r in [
            ResolverErrorReason::TldEmpty,
            ResolverErrorReason::ResolvedNotActive,
            ResolverErrorReason::ResolvConfUnreadable,
            ResolverErrorReason::DropInNotWritable(PathBuf::from("/etc/systemd/resolved.conf.d")),
        ] {
            assert!(!r.to_string().is_empty());
        }
    }

    /// Tripwire: constructing every variant of every reason enum and the
    /// outer error type. New variants drop coverage if not added here.
    #[test]
    fn construct_every_variant() {
        let _ = PlatformError::NeedsHelper { operation: "x" };
        let _ = PlatformError::Unsupported { operation: "x" };
        let _ = PlatformError::MissingHomeDir;
        let _ = PlatformError::TrustStore {
            reason: TrustStoreErrorReason::NssCertutilFailed(1),
        };
        let _ = PlatformError::Resolver {
            reason: ResolverErrorReason::TldEmpty,
        };
        let _ = PlatformError::Bind {
            port: 0,
            source: std::io::Error::from(std::io::ErrorKind::Other),
        };
        let _ = PlatformError::BindPair {
            reason: BindPairErrorReason::BothPairsFailed {
                desired_http: std::io::ErrorKind::Other,
                desired_https: std::io::ErrorKind::Other,
                fallback_http: std::io::ErrorKind::Other,
                fallback_https: std::io::ErrorKind::Other,
            },
        };
        let _ = PlatformError::Io {
            path: PathBuf::new(),
            source: std::io::Error::from(std::io::ErrorKind::Other),
        };
        let _ = PlatformError::LanIpDiscovery {
            source: std::io::Error::from(std::io::ErrorKind::Other),
        };
        let _ = PlatformError::MissingTool {
            tool: "x",
            install_hint: None,
        };
        let _ = PlatformError::MissingTool {
            tool: "x",
            install_hint: Some("y"),
        };
    }

    /// Op-tag constants must be non-empty and stable strings.
    #[test]
    fn op_tags_are_non_empty() {
        for op in [
            ops::PATHS_RESOLVE,
            ops::INSTALL_CA,
            ops::UNINSTALL_CA,
            ops::INSTALL_FIREFOX_NSS,
            ops::IS_PRESENT_SYSTEM,
            ops::IS_TRUSTED,
            ops::INSTALL_RESOLVER,
            ops::UNINSTALL_RESOLVER,
            ops::IS_INSTALLED_RESOLVER,
            ops::BIND,
            ops::BIND_PAIR,
            ops::SETCAP,
            ops::INSTALL_PORT_REDIRECT,
            ops::UNINSTALL_PORT_REDIRECT,
            ops::INSTALL_LAN_PORT_REDIRECT,
            ops::UNINSTALL_LAN_PORT_REDIRECT,
        ] {
            assert!(!op.is_empty());
        }
    }
}
