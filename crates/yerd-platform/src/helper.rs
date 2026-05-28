//! Typed helper-invocation contract.
//!
//! [`HelperInvocation`] is the typed enum the daemon hands to its
//! subprocess spawner when it sees `PlatformError::NeedsHelper`. Values
//! stay typed all the way until [`HelperInvocation::to_argv`] serialises
//! them at the spawn site — there is no `Vec<String>` round-trip in
//! between.
//!
//! The argv shape is a **wire contract** with the `yerd-helper` binary
//! and is pinned by `tests/helper_argv_shape.rs`. Adding a flag or
//! reordering trips the test.
//!
//! See `crate::error::ops` for the operation-tag constants used as the
//! first argv element.

use std::ffi::OsString;
use std::net::SocketAddr;
use std::path::PathBuf;

use crate::error::ops;
use crate::trust_store::CaFingerprint;

/// One privileged operation the daemon asks `yerd-helper` to perform.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum HelperInvocation {
    /// Install the CA at `ca_pem_path` into the system trust store.
    /// `fp` is the SHA-256 fingerprint, used by the helper to verify
    /// after install.
    InstallCa {
        /// Absolute path to a PEM file the daemon wrote with `mode=0o600`
        /// under `PlatformDirs::runtime`.
        ca_pem_path: PathBuf,
        /// SHA-256 fingerprint of the CA's DER body.
        fp: CaFingerprint,
    },
    /// Uninstall the CA identified by `fp`.
    UninstallCa {
        /// SHA-256 fingerprint of the CA's DER body.
        fp: CaFingerprint,
    },
    /// Install the resolver for `tld`, pointing at `addr`.
    InstallResolver {
        /// TLD without leading `.`.
        tld: String,
        /// IP+port the OS resolver should forward to.
        addr: SocketAddr,
    },
    /// Uninstall the resolver for `tld`.
    UninstallResolver {
        /// TLD without leading `.`.
        tld: String,
    },
    /// Apply `cap_net_bind_service=+ep` to the daemon binary (Linux).
    Setcap {
        /// Path to the `yerdd` binary that should receive the capability.
        daemon_binary: PathBuf,
    },
}

impl HelperInvocation {
    /// Serialise to argv.
    ///
    /// The first element is always the operation tag from
    /// [`crate::error::ops`]. Subsequent elements alternate `--flag` and
    /// the typed value rendered as a single argv element. Paths are
    /// passed as native `OsString`; fingerprints render as 64 lowercase
    /// hex characters; socket addresses use their `Display` form; TLDs
    /// are passed verbatim. Infallible.
    #[must_use]
    pub fn to_argv(&self) -> Vec<OsString> {
        let mut v: Vec<OsString> = Vec::new();
        match self {
            Self::InstallCa { ca_pem_path, fp } => {
                v.push(OsString::from(ops::INSTALL_CA));
                v.push(OsString::from("--pem"));
                v.push(ca_pem_path.clone().into_os_string());
                v.push(OsString::from("--fingerprint"));
                v.push(OsString::from(fp.to_hex()));
            }
            Self::UninstallCa { fp } => {
                v.push(OsString::from(ops::UNINSTALL_CA));
                v.push(OsString::from("--fingerprint"));
                v.push(OsString::from(fp.to_hex()));
            }
            Self::InstallResolver { tld, addr } => {
                v.push(OsString::from(ops::INSTALL_RESOLVER));
                v.push(OsString::from("--tld"));
                v.push(OsString::from(tld));
                v.push(OsString::from("--addr"));
                v.push(OsString::from(addr.to_string()));
            }
            Self::UninstallResolver { tld } => {
                v.push(OsString::from(ops::UNINSTALL_RESOLVER));
                v.push(OsString::from("--tld"));
                v.push(OsString::from(tld));
            }
            Self::Setcap { daemon_binary } => {
                v.push(OsString::from(ops::SETCAP));
                v.push(OsString::from("--binary"));
                v.push(daemon_binary.clone().into_os_string());
            }
        }
        v
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

    fn argv_strs(inv: &HelperInvocation) -> Vec<String> {
        inv.to_argv()
            .into_iter()
            .map(|o| o.into_string().unwrap())
            .collect()
    }

    #[test]
    fn install_ca_argv_shape() {
        let inv = HelperInvocation::InstallCa {
            ca_pem_path: PathBuf::from("/run/user/1000/yerd/ca.pem"),
            fp: CaFingerprint::new([0xAB; 32]),
        };
        assert_eq!(
            argv_strs(&inv),
            vec![
                "install-ca",
                "--pem",
                "/run/user/1000/yerd/ca.pem",
                "--fingerprint",
                &"ab".repeat(32),
            ]
        );
    }

    #[test]
    fn uninstall_ca_argv_shape() {
        let inv = HelperInvocation::UninstallCa {
            fp: CaFingerprint::new([0x12; 32]),
        };
        assert_eq!(
            argv_strs(&inv),
            vec!["uninstall-ca", "--fingerprint", &"12".repeat(32)]
        );
    }

    #[test]
    fn install_resolver_argv_shape() {
        let inv = HelperInvocation::InstallResolver {
            tld: "test".to_string(),
            addr: "127.0.0.1:5353".parse().unwrap(),
        };
        assert_eq!(
            argv_strs(&inv),
            vec![
                "install-resolver",
                "--tld",
                "test",
                "--addr",
                "127.0.0.1:5353"
            ]
        );
    }

    #[test]
    fn uninstall_resolver_argv_shape() {
        let inv = HelperInvocation::UninstallResolver {
            tld: "test".to_string(),
        };
        assert_eq!(argv_strs(&inv), vec!["uninstall-resolver", "--tld", "test"]);
    }

    #[test]
    fn setcap_argv_shape() {
        let inv = HelperInvocation::Setcap {
            daemon_binary: PathBuf::from("/usr/bin/yerdd"),
        };
        assert_eq!(
            argv_strs(&inv),
            vec!["setcap", "--binary", "/usr/bin/yerdd"]
        );
    }

    #[test]
    fn fingerprint_in_argv_is_lowercase_hex_64_chars() {
        let inv = HelperInvocation::UninstallCa {
            fp: CaFingerprint::new([0xFF; 32]),
        };
        let v = argv_strs(&inv);
        let fp_str = &v[2];
        assert_eq!(fp_str.len(), 64);
        assert!(fp_str.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(fp_str.chars().all(|c| !c.is_ascii_uppercase()));
    }

    #[test]
    fn ipv6_address_renders_via_display() {
        let inv = HelperInvocation::InstallResolver {
            tld: "test".to_string(),
            addr: "[::1]:53".parse().unwrap(),
        };
        let v = argv_strs(&inv);
        assert_eq!(v[4], "[::1]:53");
    }
}
