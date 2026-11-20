//! Clap-derived CLI plus a debug-build cross-check against
//! [`yerd_platform::HelperInvocation::from_argv`].
//!
//! The cross-check guards against silent wire-contract drift: if a
//! clap upgrade ever normalises argv differently from `from_argv`'s
//! alternating-flag parser, dev/CI builds fire `WireDrift` on the next
//! invocation. The check is gated on `cfg(debug_assertions)` so a
//! benign clap upgrade cannot brick shipped release binaries.

#![allow(clippy::similar_names)]

use std::ffi::OsString;
use std::net::SocketAddr;
use std::path::PathBuf;

use clap::Parser;
use yerd_platform::error::ops;
use yerd_platform::{CaFingerprint, HelperInvocation};

use crate::error::{HelperError, ValidationReason};

/// Top-level CLI.
#[derive(Debug, Parser)]
#[command(
    name = "yerd-helper",
    about = "Privileged one-shot for Yerd",
    version,
    propagate_version = true,
    arg_required_else_help = true,
    disable_help_subcommand = true
)]
pub struct Cli {
    /// **Debug builds only.** Bypass the effective-UID check. The
    /// field does not exist in release builds (compiled out by
    /// `cfg(debug_assertions)`), so this flag cannot be passed at
    /// runtime to a shipped binary.
    #[cfg(debug_assertions)]
    #[arg(long, global = true, hide = true)]
    pub skip_priv_check: bool,

    #[command(subcommand)]
    pub op: Op,
}

/// Subcommand mirror of `HelperInvocation`.
#[derive(Debug, clap::Subcommand)]
pub enum Op {
    /// Install a CA into the system trust store.
    InstallCa {
        /// Absolute path to a PEM file containing one CERTIFICATE.
        #[arg(long, value_name = "PATH")]
        pem: PathBuf,
        /// SHA-256 fingerprint as 64 lowercase hex chars.
        #[arg(long, value_name = "HEX")]
        fingerprint: String,
    },
    /// Uninstall a CA from the system trust store.
    UninstallCa {
        /// SHA-256 fingerprint as 64 lowercase hex chars.
        #[arg(long, value_name = "HEX")]
        fingerprint: String,
    },
    /// Install a `.test` resolver redirect.
    InstallResolver {
        /// TLD without leading dot (e.g. `test`).
        #[arg(long, value_name = "NAME")]
        tld: String,
        /// IP+port to forward queries to.
        #[arg(long, value_name = "SOCKETADDR")]
        addr: SocketAddr,
    },
    /// Remove a `.test` resolver redirect.
    UninstallResolver {
        /// TLD without leading dot.
        #[arg(long, value_name = "NAME")]
        tld: String,
    },
    /// Apply `cap_net_bind_service` to the daemon binary (Linux only).
    Setcap {
        /// Absolute path to the daemon binary (must basename to
        /// `yerdd`).
        #[arg(long, value_name = "PATH")]
        binary: PathBuf,
    },
    /// Install the launchd port forwarder 127.0.0.1:80/443 → rootless ports
    /// (macOS only).
    InstallPortRedirect {
        /// Privileged HTTP port to redirect from (80).
        #[arg(long, value_name = "PORT")]
        http_from: u16,
        /// Rootless HTTP port the daemon listens on.
        #[arg(long, value_name = "PORT")]
        http_to: u16,
        /// Privileged HTTPS port to redirect from (443).
        #[arg(long, value_name = "PORT")]
        https_from: u16,
        /// Rootless HTTPS port the daemon listens on.
        #[arg(long, value_name = "PORT")]
        https_to: u16,
        /// The non-root user the forwarder runs as.
        #[arg(long, value_name = "UID", value_parser = clap::value_parser!(u32).range(1..))]
        uid: u32,
    },
    /// Remove the loopback port forwarder and any legacy pf redirect (macOS
    /// only).
    UninstallPortRedirect,
    /// Install the LAN port forwarder 0.0.0.0:80/443 → rootless ports (macOS
    /// only).
    InstallLanPortRedirect {
        /// Privileged HTTP port to redirect from (80).
        #[arg(long, value_name = "PORT")]
        http_from: u16,
        /// Rootless HTTP port the daemon listens on.
        #[arg(long, value_name = "PORT")]
        http_to: u16,
        /// Privileged HTTPS port to redirect from (443).
        #[arg(long, value_name = "PORT")]
        https_from: u16,
        /// Rootless HTTPS port the daemon listens on.
        #[arg(long, value_name = "PORT")]
        https_to: u16,
        /// The non-root user the forwarder runs as.
        #[arg(long, value_name = "UID", value_parser = clap::value_parser!(u32).range(1..))]
        uid: u32,
    },
    /// Remove the LAN port forwarder and any legacy pf redirect (macOS only).
    UninstallLanPortRedirect,
    /// Register the headless daemon as a system-domain launchd job running as
    /// `uid` (macOS only).
    InstallDaemonJob {
        /// The non-root user the daemon runs as.
        #[arg(long, value_name = "UID", value_parser = clap::value_parser!(u32).range(1..))]
        uid: u32,
    },
    /// Remove the headless daemon's launchd job (macOS only).
    UninstallDaemonJob,
}

/// Parse argv into a typed [`HelperInvocation`] (plus the
/// `--skip-priv-check` flag in debug builds).
pub fn parse<I, T>(args: I) -> Result<ParsedCli, HelperError>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let argv: Vec<OsString> = args.into_iter().map(Into::into).collect();
    let cli = Cli::try_parse_from(&argv).map_err(|e| HelperError::ArgvUsage(e.to_string()))?;
    let skip_priv_check = skip_priv_check_value(&cli);
    let invocation = cli.op.into_invocation()?;

    #[cfg(debug_assertions)]
    {
        if argv.len() > 1 {
            let tail: Vec<OsString> = argv.iter().skip(1).cloned().collect();
            let filtered: Vec<OsString> = tail
                .into_iter()
                .filter(|a| a != "--skip-priv-check")
                .collect();
            if let Ok(parsed) = HelperInvocation::from_argv(&filtered) {
                if invocation_tag(&parsed) != invocation_tag(&invocation) {
                    return Err(HelperError::WireDrift {
                        clap: invocation_tag(&invocation),
                        from_argv: invocation_tag(&parsed),
                    });
                }
            }
        }
    }

    Ok(ParsedCli {
        invocation,
        skip_priv_check,
    })
}

#[cfg(debug_assertions)]
fn skip_priv_check_value(cli: &Cli) -> bool {
    cli.skip_priv_check
}

#[cfg(not(debug_assertions))]
fn skip_priv_check_value(_cli: &Cli) -> bool {
    false
}

/// Result of parsing argv.
#[derive(Debug)]
pub struct ParsedCli {
    /// The typed operation to perform.
    pub invocation: HelperInvocation,
    /// Whether the debug-only `--skip-priv-check` flag was passed.
    pub skip_priv_check: bool,
}

impl Op {
    fn into_invocation(self) -> Result<HelperInvocation, HelperError> {
        match self {
            Self::InstallCa { pem, fingerprint } => Ok(HelperInvocation::InstallCa {
                ca_pem_path: pem,
                fp: parse_fingerprint_str(&fingerprint)?,
            }),
            Self::UninstallCa { fingerprint } => Ok(HelperInvocation::UninstallCa {
                fp: parse_fingerprint_str(&fingerprint)?,
            }),
            Self::InstallResolver { tld, addr } => {
                Ok(HelperInvocation::InstallResolver { tld, addr })
            }
            Self::UninstallResolver { tld } => Ok(HelperInvocation::UninstallResolver { tld }),
            Self::Setcap { binary } => Ok(HelperInvocation::Setcap {
                daemon_binary: binary,
            }),
            Self::InstallPortRedirect {
                http_from,
                http_to,
                https_from,
                https_to,
                uid,
            } => Ok(HelperInvocation::InstallPortRedirect {
                http_from,
                http_to,
                https_from,
                https_to,
                uid,
            }),
            Self::UninstallPortRedirect => Ok(HelperInvocation::UninstallPortRedirect),
            Self::InstallLanPortRedirect {
                http_from,
                http_to,
                https_from,
                https_to,
                uid,
            } => Ok(HelperInvocation::InstallLanPortRedirect {
                http_from,
                http_to,
                https_from,
                https_to,
                uid,
            }),
            Self::UninstallLanPortRedirect => Ok(HelperInvocation::UninstallLanPortRedirect),
            Self::InstallDaemonJob { uid } => Ok(HelperInvocation::InstallDaemonJob { uid }),
            Self::UninstallDaemonJob => Ok(HelperInvocation::UninstallDaemonJob),
        }
    }
}

fn parse_fingerprint_str(s: &str) -> Result<CaFingerprint, HelperError> {
    if s.len() != 64
        || s.chars()
            .any(|c| !c.is_ascii_hexdigit() || c.is_ascii_uppercase())
    {
        return Err(HelperError::Validation {
            reason: ValidationReason::BadFingerprintHex,
        });
    }
    let bytes = hex::decode(s).map_err(|_| HelperError::Validation {
        reason: ValidationReason::BadFingerprintHex,
    })?;
    let arr: [u8; 32] = bytes.try_into().map_err(|_| HelperError::Validation {
        reason: ValidationReason::BadFingerprintHex,
    })?;
    Ok(CaFingerprint::new(arr))
}

fn invocation_tag(inv: &HelperInvocation) -> &'static str {
    match inv {
        HelperInvocation::InstallCa { .. } => ops::INSTALL_CA,
        HelperInvocation::UninstallCa { .. } => ops::UNINSTALL_CA,
        HelperInvocation::InstallResolver { .. } => ops::INSTALL_RESOLVER,
        HelperInvocation::UninstallResolver { .. } => ops::UNINSTALL_RESOLVER,
        HelperInvocation::Setcap { .. } => ops::SETCAP,
        HelperInvocation::InstallPortRedirect { .. } => ops::INSTALL_PORT_REDIRECT,
        HelperInvocation::UninstallPortRedirect => ops::UNINSTALL_PORT_REDIRECT,
        HelperInvocation::InstallLanPortRedirect { .. } => ops::INSTALL_LAN_PORT_REDIRECT,
        HelperInvocation::UninstallLanPortRedirect => ops::UNINSTALL_LAN_PORT_REDIRECT,
        HelperInvocation::InstallDaemonJob { .. } => ops::INSTALL_DAEMON_JOB,
        HelperInvocation::UninstallDaemonJob => ops::UNINSTALL_DAEMON_JOB,
        _ => "unknown",
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

    fn parse_ok(args: &[&str]) -> ParsedCli {
        let with_bin = std::iter::once(OsString::from("yerd-helper"))
            .chain(args.iter().map(|s| OsString::from(*s)));
        parse(with_bin).expect("expected successful parse")
    }

    fn parse_err(args: &[&str]) -> HelperError {
        let with_bin = std::iter::once(OsString::from("yerd-helper"))
            .chain(args.iter().map(|s| OsString::from(*s)));
        parse(with_bin).expect_err("expected parse error")
    }

    #[test]
    fn install_ca_parses() {
        let p = parse_ok(&[
            "install-ca",
            "--pem",
            "/x/ca.pem",
            "--fingerprint",
            &"ab".repeat(32),
        ]);
        assert!(matches!(p.invocation, HelperInvocation::InstallCa { .. }));
    }

    #[test]
    fn install_ca_bad_fingerprint_rejected() {
        let err = parse_err(&["install-ca", "--pem", "/x/ca.pem", "--fingerprint", "AB"]);
        assert!(matches!(
            err,
            HelperError::Validation {
                reason: ValidationReason::BadFingerprintHex
            }
        ));
    }

    #[test]
    fn unknown_subcommand_rejected_as_usage() {
        let err = parse_err(&["bogus"]);
        assert!(matches!(err, HelperError::ArgvUsage(_)));
    }

    #[test]
    fn unknown_flag_rejected_as_usage() {
        let err = parse_err(&[
            "install-ca",
            "--pem",
            "/x",
            "--fingerprint",
            &"ab".repeat(32),
            "--rogue",
            "y",
        ]);
        assert!(matches!(err, HelperError::ArgvUsage(_)));
    }

    #[test]
    fn install_resolver_parses() {
        let p = parse_ok(&[
            "install-resolver",
            "--tld",
            "test",
            "--addr",
            "127.0.0.1:5353",
        ]);
        match p.invocation {
            HelperInvocation::InstallResolver { tld, addr } => {
                assert_eq!(tld, "test");
                assert_eq!(addr.to_string(), "127.0.0.1:5353");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn install_resolver_bad_addr_rejected_as_usage() {
        let err = parse_err(&["install-resolver", "--tld", "test", "--addr", "not-an-addr"]);
        assert!(matches!(err, HelperError::ArgvUsage(_)));
    }

    #[test]
    fn port_redirect_uid_zero_rejected_as_usage() {
        for op in ["install-port-redirect", "install-lan-port-redirect"] {
            let err = parse_err(&[
                op,
                "--http-from",
                "80",
                "--http-to",
                "8080",
                "--https-from",
                "443",
                "--https-to",
                "8443",
                "--uid",
                "0",
            ]);
            assert!(matches!(err, HelperError::ArgvUsage(_)), "{op}: {err:?}");
        }
    }

    #[test]
    fn daemon_job_ops_parse() {
        let p = parse_ok(&["install-daemon-job", "--uid", "501"]);
        assert!(matches!(
            p.invocation,
            HelperInvocation::InstallDaemonJob { uid: 501 }
        ));
        let p = parse_ok(&["uninstall-daemon-job"]);
        assert!(matches!(p.invocation, HelperInvocation::UninstallDaemonJob));
    }

    #[test]
    fn daemon_job_bad_argv_rejected_as_usage() {
        for args in [
            &["install-daemon-job"][..],
            &["install-daemon-job", "--uid", "0"],
            &["install-daemon-job", "--uid", "abc"],
            &["uninstall-daemon-job", "--uid", "501"],
        ] {
            let err = parse_err(args);
            assert!(
                matches!(err, HelperError::ArgvUsage(_)),
                "{args:?}: {err:?}"
            );
        }
    }

    #[test]
    fn setcap_parses() {
        let p = parse_ok(&["setcap", "--binary", "/usr/bin/yerdd"]);
        assert!(matches!(p.invocation, HelperInvocation::Setcap { .. }));
    }

    #[cfg(debug_assertions)]
    #[test]
    fn skip_priv_check_recognised_in_debug() {
        let p = parse_ok(&["--skip-priv-check", "uninstall-resolver", "--tld", "test"]);
        assert!(p.skip_priv_check);
    }

    #[test]
    fn skip_priv_check_defaults_false() {
        let p = parse_ok(&["uninstall-resolver", "--tld", "test"]);
        assert!(!p.skip_priv_check);
    }
}
