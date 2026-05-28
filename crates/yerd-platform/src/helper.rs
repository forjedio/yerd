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

use std::ffi::{OsStr, OsString};
use std::net::SocketAddr;
use std::path::PathBuf;

use crate::error::ops;
use crate::trust_store::CaFingerprint;

/// Failures from [`HelperInvocation::from_argv`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ArgvParseError {
    /// argv vector was empty.
    #[error("argv empty")]
    Empty,
    /// First argv element did not match any known operation tag.
    #[error("unknown operation tag: {0:?}")]
    UnknownOp(OsString),
    /// Required flag missing for the given operation.
    #[error("missing required flag for {op}: {flag}")]
    MissingFlag {
        /// Operation tag from `error::ops`.
        op: &'static str,
        /// Flag that was expected (e.g. `--fingerprint`).
        flag: &'static str,
    },
    /// Flag received that doesn't belong to this operation.
    #[error("unknown flag for {op}: {flag:?}")]
    UnknownFlag {
        /// Operation tag from `error::ops`.
        op: &'static str,
        /// The unexpected flag as observed in argv.
        flag: OsString,
    },
    /// Flag appeared but no value followed it.
    #[error("missing value after flag {flag}")]
    MissingValue {
        /// The flag in question.
        flag: &'static str,
    },
    /// Fingerprint did not parse as 64 lowercase hex chars.
    #[error("invalid fingerprint hex (need 64 lowercase hex chars)")]
    BadFingerprint,
    /// Socket address did not parse.
    #[error("invalid socket address: {0:?}")]
    BadAddr(OsString),
    /// Trailing argv after the parser had consumed every expected flag.
    #[error("trailing argv after parse: {0:?}")]
    Trailing(OsString),
    /// A non-UTF-8 value was supplied where a UTF-8 string was needed
    /// (e.g. fingerprint, socketaddr, TLD).
    #[error("non-utf8 value for flag {flag}")]
    NonUtf8 {
        /// The flag whose value was non-UTF-8.
        flag: &'static str,
    },
}

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
    /// Parse an argv vector (as produced by [`Self::to_argv`]) back
    /// into the typed enum.
    ///
    /// The first element is the operation tag; subsequent elements
    /// alternate `--flag` and a typed value. The parser is strict —
    /// unknown flags, missing values, and trailing argv are rejected.
    /// This pairs with `to_argv`; both sides of the wire are
    /// round-trip tested in `tests/helper_argv_roundtrip.rs`.
    pub fn from_argv(argv: &[OsString]) -> Result<Self, ArgvParseError> {
        let (head, rest) = argv.split_first().ok_or(ArgvParseError::Empty)?;
        let op = head
            .to_str()
            .ok_or_else(|| ArgvParseError::UnknownOp(head.clone()))?;
        match op {
            t if t == ops::INSTALL_CA => parse_install_ca(rest),
            t if t == ops::UNINSTALL_CA => parse_uninstall_ca(rest),
            t if t == ops::INSTALL_RESOLVER => parse_install_resolver(rest),
            t if t == ops::UNINSTALL_RESOLVER => parse_uninstall_resolver(rest),
            t if t == ops::SETCAP => parse_setcap(rest),
            _ => Err(ArgvParseError::UnknownOp(head.clone())),
        }
    }

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

// ---- per-op argv parsers ----------------------------------------------------

/// Pull the next argv element as `(flag, value)`, returning a typed
/// error if the flag is absent or the value is missing.
fn next_pair<'a>(iter: &mut std::slice::Iter<'a, OsString>) -> Option<(&'a OsStr, &'a OsString)> {
    let flag = iter.next()?;
    Some((flag.as_os_str(), iter.next()?))
}

fn require_utf8(value: &OsStr, flag: &'static str) -> Result<String, ArgvParseError> {
    value
        .to_str()
        .map(str::to_owned)
        .ok_or(ArgvParseError::NonUtf8 { flag })
}

fn parse_fingerprint(value: &OsStr) -> Result<CaFingerprint, ArgvParseError> {
    let s = value.to_str().ok_or(ArgvParseError::BadFingerprint)?;
    if s.len() != 64
        || s.chars()
            .any(|c| !c.is_ascii_hexdigit() || c.is_ascii_uppercase())
    {
        return Err(ArgvParseError::BadFingerprint);
    }
    let bytes = hex::decode(s).map_err(|_| ArgvParseError::BadFingerprint)?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| ArgvParseError::BadFingerprint)?;
    Ok(CaFingerprint::new(arr))
}

fn parse_install_ca(rest: &[OsString]) -> Result<HelperInvocation, ArgvParseError> {
    let mut pem: Option<PathBuf> = None;
    let mut fp: Option<CaFingerprint> = None;
    let mut iter = rest.iter();
    while let Some((flag, value)) = next_pair(&mut iter) {
        match flag.to_str() {
            Some("--pem") => pem = Some(PathBuf::from(value)),
            Some("--fingerprint") => fp = Some(parse_fingerprint(value)?),
            _ => {
                return Err(ArgvParseError::UnknownFlag {
                    op: ops::INSTALL_CA,
                    flag: flag.to_owned(),
                });
            }
        }
    }
    if let Some(trailing) = iter.next() {
        return Err(ArgvParseError::Trailing(trailing.clone()));
    }
    let pem = pem.ok_or(ArgvParseError::MissingFlag {
        op: ops::INSTALL_CA,
        flag: "--pem",
    })?;
    let fp = fp.ok_or(ArgvParseError::MissingFlag {
        op: ops::INSTALL_CA,
        flag: "--fingerprint",
    })?;
    Ok(HelperInvocation::InstallCa {
        ca_pem_path: pem,
        fp,
    })
}

fn parse_uninstall_ca(rest: &[OsString]) -> Result<HelperInvocation, ArgvParseError> {
    let mut fp: Option<CaFingerprint> = None;
    let mut iter = rest.iter();
    while let Some((flag, value)) = next_pair(&mut iter) {
        match flag.to_str() {
            Some("--fingerprint") => fp = Some(parse_fingerprint(value)?),
            _ => {
                return Err(ArgvParseError::UnknownFlag {
                    op: ops::UNINSTALL_CA,
                    flag: flag.to_owned(),
                });
            }
        }
    }
    if let Some(trailing) = iter.next() {
        return Err(ArgvParseError::Trailing(trailing.clone()));
    }
    let fp = fp.ok_or(ArgvParseError::MissingFlag {
        op: ops::UNINSTALL_CA,
        flag: "--fingerprint",
    })?;
    Ok(HelperInvocation::UninstallCa { fp })
}

fn parse_install_resolver(rest: &[OsString]) -> Result<HelperInvocation, ArgvParseError> {
    let mut tld: Option<String> = None;
    let mut addr: Option<SocketAddr> = None;
    let mut iter = rest.iter();
    while let Some((flag, value)) = next_pair(&mut iter) {
        match flag.to_str() {
            Some("--tld") => tld = Some(require_utf8(value, "--tld")?),
            Some("--addr") => {
                let s = require_utf8(value, "--addr")?;
                addr = Some(
                    s.parse()
                        .map_err(|_| ArgvParseError::BadAddr(value.clone()))?,
                );
            }
            _ => {
                return Err(ArgvParseError::UnknownFlag {
                    op: ops::INSTALL_RESOLVER,
                    flag: flag.to_owned(),
                });
            }
        }
    }
    if let Some(trailing) = iter.next() {
        return Err(ArgvParseError::Trailing(trailing.clone()));
    }
    let tld = tld.ok_or(ArgvParseError::MissingFlag {
        op: ops::INSTALL_RESOLVER,
        flag: "--tld",
    })?;
    let addr = addr.ok_or(ArgvParseError::MissingFlag {
        op: ops::INSTALL_RESOLVER,
        flag: "--addr",
    })?;
    Ok(HelperInvocation::InstallResolver { tld, addr })
}

fn parse_uninstall_resolver(rest: &[OsString]) -> Result<HelperInvocation, ArgvParseError> {
    let mut tld: Option<String> = None;
    let mut iter = rest.iter();
    while let Some((flag, value)) = next_pair(&mut iter) {
        match flag.to_str() {
            Some("--tld") => tld = Some(require_utf8(value, "--tld")?),
            _ => {
                return Err(ArgvParseError::UnknownFlag {
                    op: ops::UNINSTALL_RESOLVER,
                    flag: flag.to_owned(),
                });
            }
        }
    }
    if let Some(trailing) = iter.next() {
        return Err(ArgvParseError::Trailing(trailing.clone()));
    }
    let tld = tld.ok_or(ArgvParseError::MissingFlag {
        op: ops::UNINSTALL_RESOLVER,
        flag: "--tld",
    })?;
    Ok(HelperInvocation::UninstallResolver { tld })
}

fn parse_setcap(rest: &[OsString]) -> Result<HelperInvocation, ArgvParseError> {
    let mut binary: Option<PathBuf> = None;
    let mut iter = rest.iter();
    while let Some((flag, value)) = next_pair(&mut iter) {
        match flag.to_str() {
            Some("--binary") => binary = Some(PathBuf::from(value)),
            _ => {
                return Err(ArgvParseError::UnknownFlag {
                    op: ops::SETCAP,
                    flag: flag.to_owned(),
                });
            }
        }
    }
    if let Some(trailing) = iter.next() {
        return Err(ArgvParseError::Trailing(trailing.clone()));
    }
    let binary = binary.ok_or(ArgvParseError::MissingFlag {
        op: ops::SETCAP,
        flag: "--binary",
    })?;
    Ok(HelperInvocation::Setcap {
        daemon_binary: binary,
    })
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

    // ---- from_argv parser tests --------------------------------------

    fn argv(elements: &[&str]) -> Vec<OsString> {
        elements.iter().map(|s| OsString::from(*s)).collect()
    }

    #[test]
    fn from_argv_empty_rejected() {
        assert!(matches!(
            HelperInvocation::from_argv(&[]),
            Err(ArgvParseError::Empty)
        ));
    }

    #[test]
    fn from_argv_unknown_op_rejected() {
        let v = argv(&["bogus-op"]);
        assert!(matches!(
            HelperInvocation::from_argv(&v),
            Err(ArgvParseError::UnknownOp(_))
        ));
    }

    #[test]
    fn from_argv_install_ca_happy_path() {
        let v = argv(&[
            "install-ca",
            "--pem",
            "/run/yerd/ca.pem",
            "--fingerprint",
            &"ab".repeat(32),
        ]);
        let inv = HelperInvocation::from_argv(&v).unwrap();
        match inv {
            HelperInvocation::InstallCa { ca_pem_path, fp } => {
                assert_eq!(ca_pem_path, PathBuf::from("/run/yerd/ca.pem"));
                assert_eq!(fp, CaFingerprint::new([0xAB; 32]));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn from_argv_install_ca_missing_pem() {
        let v = argv(&["install-ca", "--fingerprint", &"ab".repeat(32)]);
        let err = HelperInvocation::from_argv(&v).unwrap_err();
        assert!(matches!(
            err,
            ArgvParseError::MissingFlag { flag: "--pem", .. }
        ));
    }

    #[test]
    fn from_argv_install_ca_missing_fingerprint() {
        let v = argv(&["install-ca", "--pem", "/x"]);
        let err = HelperInvocation::from_argv(&v).unwrap_err();
        assert!(matches!(
            err,
            ArgvParseError::MissingFlag {
                flag: "--fingerprint",
                ..
            }
        ));
    }

    #[test]
    fn from_argv_install_ca_bad_fingerprint_short() {
        let v = argv(&["install-ca", "--pem", "/x", "--fingerprint", "abc"]);
        assert!(matches!(
            HelperInvocation::from_argv(&v),
            Err(ArgvParseError::BadFingerprint)
        ));
    }

    #[test]
    fn from_argv_install_ca_bad_fingerprint_uppercase() {
        let v = argv(&[
            "install-ca",
            "--pem",
            "/x",
            "--fingerprint",
            &"AB".repeat(32),
        ]);
        assert!(matches!(
            HelperInvocation::from_argv(&v),
            Err(ArgvParseError::BadFingerprint)
        ));
    }

    #[test]
    fn from_argv_install_ca_unknown_flag() {
        let v = argv(&[
            "install-ca",
            "--pem",
            "/x",
            "--fingerprint",
            &"ab".repeat(32),
            "--rogue",
            "y",
        ]);
        let err = HelperInvocation::from_argv(&v).unwrap_err();
        assert!(matches!(err, ArgvParseError::UnknownFlag { .. }));
    }

    #[test]
    fn from_argv_uninstall_ca_happy_path() {
        let v = argv(&["uninstall-ca", "--fingerprint", &"12".repeat(32)]);
        let inv = HelperInvocation::from_argv(&v).unwrap();
        assert!(matches!(inv, HelperInvocation::UninstallCa { .. }));
    }

    #[test]
    fn from_argv_install_resolver_happy_path() {
        let v = argv(&[
            "install-resolver",
            "--tld",
            "test",
            "--addr",
            "127.0.0.1:5353",
        ]);
        let inv = HelperInvocation::from_argv(&v).unwrap();
        match inv {
            HelperInvocation::InstallResolver { tld, addr } => {
                assert_eq!(tld, "test");
                assert_eq!(addr.to_string(), "127.0.0.1:5353");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn from_argv_install_resolver_bad_addr() {
        let v = argv(&["install-resolver", "--tld", "test", "--addr", "not-an-addr"]);
        assert!(matches!(
            HelperInvocation::from_argv(&v),
            Err(ArgvParseError::BadAddr(_))
        ));
    }

    #[test]
    fn from_argv_uninstall_resolver_happy_path() {
        let v = argv(&["uninstall-resolver", "--tld", "test"]);
        let inv = HelperInvocation::from_argv(&v).unwrap();
        assert!(matches!(inv, HelperInvocation::UninstallResolver { .. }));
    }

    #[test]
    fn from_argv_setcap_happy_path() {
        let v = argv(&["setcap", "--binary", "/usr/bin/yerdd"]);
        let inv = HelperInvocation::from_argv(&v).unwrap();
        match inv {
            HelperInvocation::Setcap { daemon_binary } => {
                assert_eq!(daemon_binary, PathBuf::from("/usr/bin/yerdd"));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn from_argv_setcap_missing_value_after_flag() {
        // "--binary" with no following value
        let v = argv(&["setcap", "--binary"]);
        // The pair-iterator yields nothing for an odd-length tail, so
        // we fall through to MissingFlag.
        let err = HelperInvocation::from_argv(&v).unwrap_err();
        assert!(matches!(
            err,
            ArgvParseError::MissingFlag {
                flag: "--binary",
                ..
            }
        ));
    }

    #[test]
    fn roundtrip_install_ca() {
        let inv = HelperInvocation::InstallCa {
            ca_pem_path: PathBuf::from("/x/ca.pem"),
            fp: CaFingerprint::new([0xCD; 32]),
        };
        let argv = inv.to_argv();
        let parsed = HelperInvocation::from_argv(&argv).unwrap();
        match (inv, parsed) {
            (
                HelperInvocation::InstallCa {
                    ca_pem_path: orig_path,
                    fp: orig_fp,
                },
                HelperInvocation::InstallCa {
                    ca_pem_path: back_path,
                    fp: back_fp,
                },
            ) => {
                assert_eq!(orig_path, back_path);
                assert_eq!(orig_fp, back_fp);
            }
            _ => panic!("variant mismatch"),
        }
    }

    #[test]
    fn roundtrip_every_variant() {
        let cases: &[HelperInvocation] = &[
            HelperInvocation::InstallCa {
                ca_pem_path: PathBuf::from("/x"),
                fp: CaFingerprint::new([1u8; 32]),
            },
            HelperInvocation::UninstallCa {
                fp: CaFingerprint::new([2u8; 32]),
            },
            HelperInvocation::InstallResolver {
                tld: "test".into(),
                addr: "127.0.0.1:53".parse().unwrap(),
            },
            HelperInvocation::UninstallResolver { tld: "test".into() },
            HelperInvocation::Setcap {
                daemon_binary: PathBuf::from("/usr/bin/yerdd"),
            },
        ];
        for inv in cases {
            let v = inv.to_argv();
            let back = HelperInvocation::from_argv(&v).expect("round-trip parse");
            // Re-serialise both and compare argv vectors — avoids
            // needing PartialEq on HelperInvocation.
            assert_eq!(inv.to_argv(), back.to_argv());
        }
    }
}
