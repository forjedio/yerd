//! Windows OS implementation.
//!
//! Windows implements a growing subset of the traits with real `Windows*`
//! types (`Paths`, `PortBinder`, `PortRedirector`); the remainder are type
//! aliases to the `os::unsupported` stub, so those impls come for free and stay
//! total. Later phases replace one alias at a time with a real `Windows*` type
//! in the same change that adds its full trait impl (the "never half-flip"
//! rule).

#![allow(clippy::similar_names)]

use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use schannel::cert_context::CertContext;
use schannel::cert_store::{CertAdd, CertStore};

use crate::error::{ops, TerminalErrorReason, TrustStoreErrorReason};
use crate::paths::{Paths, PlatformDirs};
use crate::port_binder::{BoundPort, PortBinder, PortPair};
use crate::port_redirect::PortRedirector;
use crate::pure::{nrpt, pem_match, port_plan, win_pipe, win_terminal, win_token};
use crate::resolver::ResolverInstaller;
use crate::terminal::TerminalLauncher;
use crate::trust_store::{CaFingerprint, NssOutcome, TrustStore};
use crate::{BindPairErrorReason, PlatformError};

pub use super::unsupported::UnsupportedSystemMetrics as WindowsSystemMetrics;

/// `CREATE_NEW_CONSOLE` process-creation flag: the spawned shell gets its own
/// console window instead of inheriting the caller's (the daemon/GUI has none
/// worth sharing). Safe std `creation_flags`, no FFI.
const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

/// Real `TerminalLauncher` for Windows.
///
/// Tries Windows Terminal (`wt.exe -d <dir>`), then PowerShell, then `cmd.exe`,
/// first success wins - the same probe-list shape as the Linux impl. The pure
/// per-terminal command shapes live in [`win_terminal`]; this type owns only the
/// spawn loop.
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
        use std::os::windows::process::CommandExt as _;
        for term in win_terminal::WIN_TERMINAL_PROBES {
            let mut cmd = std::process::Command::new(term.program());
            cmd.args(term.args(path)).current_dir(path);
            if term.needs_new_console() {
                cmd.creation_flags(CREATE_NEW_CONSOLE);
            }
            if cmd.spawn().is_ok() {
                return Ok(());
            }
        }
        Err(PlatformError::Terminal {
            reason: TerminalErrorReason::NoSupportedTerminal,
        })
    }
}

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

/// Windows `PortRedirector` implementation.
///
/// Not applicable on Windows: sub-1024 binds are unprivileged, so
/// [`WindowsPortBinder`] direct-binds 80/443 and there is no pf-style redirect
/// to be "active". [`Self::is_active`] therefore returns `None` ("N/A"), the
/// same shape the Linux impl uses. The point of the real type is to inherit the
/// trait-default [`PortRedirector::foreign_web_listener`] loopback probe, which
/// is correct on any OS where Yerd serves over loopback, so the doctor can
/// detect a foreign process squatting 80/443.
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

/// The HKLM subkey (relative to `HKEY_LOCAL_MACHINE`) holding NRPT rules. Each
/// child key is one rule, named with a braced GUID.
const DNS_POLICY_CONFIG_KEY: &str =
    r"SYSTEM\CurrentControlSet\Services\Dnscache\Parameters\DnsPolicyConfig";

/// One NRPT rule's decoded values, read read-only from the registry.
struct NrptRuleValues {
    /// Braced GUID subkey name, usable verbatim as `Remove-DnsClientNrptRule -Name`.
    guid: String,
    /// The `Name` multi-sz (the namespaces, e.g. `[".test"]`).
    name: Vec<String>,
    /// The `GenericDNSServers` `REG_SZ` (the forward targets).
    servers: String,
}

/// Read every NRPT rule from `DnsPolicyConfig`, read-only. A missing key (no
/// rules ever created) yields an empty vec, not an error, matching the
/// idempotent-probe contract. Read-only HKLM access needs no elevation.
fn read_nrpt_rules() -> Vec<NrptRuleValues> {
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};
    use winreg::RegKey;

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let Ok(root) = hklm.open_subkey_with_flags(DNS_POLICY_CONFIG_KEY, KEY_READ) else {
        return Vec::new();
    };
    let mut rules = Vec::new();
    for guid in root.enum_keys().flatten() {
        let Ok(sub) = root.open_subkey_with_flags(&guid, KEY_READ) else {
            continue;
        };
        let name: Vec<String> = sub.get_value("Name").unwrap_or_default();
        let servers: String = sub.get_value("GenericDNSServers").unwrap_or_default();
        rules.push(NrptRuleValues {
            guid,
            name,
            servers,
        });
    }
    rules
}

/// Braced GUIDs of every NRPT rule whose namespace is `.tld`, regardless of the
/// server it forwards to.
///
/// The Windows helper calls this (through `yerd-platform`, so `winreg` stays out
/// of the helper's own dependency graph) to discover the rules it must remove
/// before adding a fresh one - so a stale or wrong-server `.tld` rule is always
/// replaced. Read-only, unprivileged.
#[must_use]
pub fn nrpt_guids_for_tld(tld: &str) -> Vec<String> {
    read_nrpt_rules()
        .into_iter()
        .filter(|rule| nrpt::name_matches_tld(&rule.name, tld))
        .map(|rule| rule.guid)
        .collect()
}

/// Real `ResolverInstaller` for Windows, backed by an NRPT wildcard rule.
///
/// `install`/`uninstall` return `NeedsHelper` (the elevated write goes through
/// `yerd-helper`, like Linux/macOS - the OS impl never spawns the helper).
/// `is_installed` reads the registry directly and needs no elevation.
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
    fn install(&self, _tld: &str, _addr: SocketAddr) -> Result<(), PlatformError> {
        Err(PlatformError::NeedsHelper {
            operation: ops::INSTALL_RESOLVER,
        })
    }

    fn uninstall(&self, _tld: &str) -> Result<(), PlatformError> {
        Err(PlatformError::NeedsHelper {
            operation: ops::UNINSTALL_RESOLVER,
        })
    }

    /// Whether an NRPT rule routes `.tld` to `addr`. An NRPT rule carries no
    /// port and always resolves to `<ip>:53`, so a non-53 `addr` can never be
    /// served by a rule and reports `false` (keeping doctor's "resolver
    /// installed" honest). An unspecified IPv4 (LAN mode) normalises to
    /// loopback, the address the rule actually holds; IPv6 never matches.
    fn is_installed(&self, tld: &str, addr: SocketAddr) -> Result<bool, PlatformError> {
        if addr.port() != 53 {
            return Ok(false);
        }
        let ip = match addr.ip() {
            IpAddr::V4(v4) if v4.is_unspecified() => Ipv4Addr::LOCALHOST.to_string(),
            IpAddr::V4(v4) => v4.to_string(),
            IpAddr::V6(_) => return Ok(false),
        };
        let present = read_nrpt_rules()
            .iter()
            .any(|rule| nrpt::rule_matches(&rule.name, &rule.servers, tld, &ip));
        Ok(present)
    }
}

/// Whether this process holds an elevated (High or System integrity) token.
///
/// Spawns `%SystemRoot%\System32\whoami.exe /groups /fo csv /nh` (absolute path,
/// never `PATH`) and parses the mandatory-integrity SID via the table-tested
/// [`win_token`] parser. Any spawn/exit failure reports `false` (conservative,
/// mirroring the Linux `/proc` fallback). No `unsafe`, no new crates - the
/// `GetTokenInformation(TokenElevation)` alternative is `unsafe` FFI this crate's
/// `#![forbid(unsafe_code)]` rules out.
#[must_use]
pub fn is_token_elevated() -> bool {
    let Ok(output) = std::process::Command::new(whoami_path())
        .args(["/groups", "/fo", "csv", "/nh"])
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    win_token::csv_has_elevated_integrity(&stdout)
}

/// Map a schannel/`std::io` cert-store failure to the shared typed reason. Every
/// Win32 cert-store call surfaces failures as [`std::io::Error`]; there is no
/// finer structure to preserve, so they collapse to
/// [`TrustStoreErrorReason::SystemApi`] like the macOS `security-framework`
/// errors do.
fn sys_api(e: std::io::Error) -> PlatformError {
    PlatformError::TrustStore {
        reason: TrustStoreErrorReason::SystemApi(e.to_string()),
    }
}

/// Add a DER certificate to `store`, replacing any existing copy of the same
/// cert. Takes `&mut CertStore` because schannel's `add_cert` mutates the store.
/// Adding to a real "Root" store raises the Windows confirmation dialog.
fn add_der(store: &mut CertStore, der: &[u8]) -> Result<(), PlatformError> {
    let cx = CertContext::new(der).map_err(sys_api)?;
    store
        .add_cert(&cx, CertAdd::ReplaceExisting)
        .map(|_| ())
        .map_err(sys_api)
}

/// Every certificate in `store` whose DER SHA-256 equals `fp`. The `certs()`
/// iterator hands out cloned contexts, so the returned owned contexts stay valid
/// after the borrow of `store` ends (and can be deleted without mid-iteration
/// invalidation).
fn find_by_fp(store: &CertStore, fp: &CaFingerprint) -> Vec<CertContext> {
    store
        .certs()
        .filter(|cx| pem_match::sha256(cx.to_der()) == *fp.as_bytes())
        .collect()
}

/// Concatenated PEM of every certificate in `store`, newline-separated. Used to
/// render a Root store's public roots for the PHP CA bundle.
fn store_root_pem(store: &CertStore) -> String {
    let mut pem = String::new();
    for cx in store.certs() {
        pem.push_str(&pem_match::der_to_pem(cx.to_der()));
        if !pem.ends_with('\n') {
            pem.push('\n');
        }
    }
    pem
}

/// Real `TrustStore` for Windows, backed by the `CurrentUser` "Root" store.
///
/// Unlike macOS/Linux, install/uninstall are performed **directly and without
/// elevation** here rather than returning `NeedsHelper`: adding a CA to the
/// `CurrentUser` Root store needs no admin rights, only a one-time OS confirmation
/// dialog. That dialog requires an **interactive desktop**, so these mutations
/// must run in the CLI or GUI process and NEVER in `yerdd` (Phase 5 turns the
/// daemon into a session-0 service with no desktop).
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
    /// Install `ca_pem` into the `CurrentUser` Root store.
    ///
    /// The fingerprint is verified against the exact DER bytes about to be
    /// imported (the integrity gate the macOS helper flow uses) before any store
    /// is opened, so a mismatch fails without side effects. Pops the Windows
    /// root-store confirmation dialog; a user decline surfaces as an error.
    /// Must run in an interactive session, never from the daemon.
    fn install_system(&self, ca_pem: &str, fp: &CaFingerprint) -> Result<(), PlatformError> {
        let der = pem_match::first_cert_der(ca_pem.as_bytes()).ok_or_else(|| {
            PlatformError::TrustStore {
                reason: TrustStoreErrorReason::SystemApi("CA PEM has no certificate".to_owned()),
            }
        })?;
        if pem_match::sha256(&der) != *fp.as_bytes() {
            return Err(PlatformError::TrustStore {
                reason: TrustStoreErrorReason::SystemApi(
                    "CA PEM does not match the expected fingerprint".to_owned(),
                ),
            });
        }
        let mut store = CertStore::open_current_user("Root").map_err(sys_api)?;
        add_der(&mut store, &der)
    }

    /// Remove every `CurrentUser`-Root certificate matching `fp`. Idempotent: zero
    /// matches is `Ok(())`. Each deletion pops its own confirmation dialog.
    fn uninstall_system(&self, fp: &CaFingerprint) -> Result<(), PlatformError> {
        let store = CertStore::open_current_user("Root").map_err(sys_api)?;
        for cx in find_by_fp(&store, fp) {
            cx.delete().map_err(sys_api)?;
        }
        Ok(())
    }

    /// Whether a certificate matching `fp` is present in the `CurrentUser` Root
    /// store. Read-only, no dialog.
    fn is_present_system(&self, fp: &CaFingerprint) -> Result<bool, PlatformError> {
        let store = CertStore::open_current_user("Root").map_err(sys_api)?;
        Ok(!find_by_fp(&store, fp).is_empty())
    }

    fn is_trusted(&self, _ca_path: &Path, fp: &CaFingerprint) -> Result<bool, PlatformError> {
        // On Windows, presence in the Root store *is* trust (there is no separate
        // trust-settings layer like macOS), so the effective-trust probe is the
        // same as the presence probe. `ca_path` is unused here.
        self.is_present_system(fp)
    }

    /// Firefox/NSS trust on Windows is a Phase 6 TODO (locked out of scope);
    /// Chromium-family browsers follow the system Root store this impl manages.
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

    /// Public roots from `LocalMachine` Root + `CurrentUser` Root as a single PEM,
    /// for the PHP CA bundle. Each store open is best-effort (a failed open is
    /// skipped, not fatal); read-only, so no admin is needed even for the
    /// `LocalMachine` reads. `Ok(None)` when neither store yields a certificate.
    fn system_root_bundle(&self) -> Result<Option<String>, PlatformError> {
        let mut pem = String::new();
        for store in [
            CertStore::open_local_machine("Root"),
            CertStore::open_current_user("Root"),
        ]
        .into_iter()
        .flatten()
        {
            pem.push_str(&store_root_pem(&store));
        }
        if pem.trim().is_empty() {
            Ok(None)
        } else {
            Ok(Some(pem))
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

/// The registry sub-path (under `HKEY_CURRENT_USER`) holding the user's own
/// environment variables, including `Path`. The invoking user's own hive, so
/// reads and writes here cross no privilege boundary (the same trust level as
/// editing `~/.zshrc` on Unix).
const HKCU_ENVIRONMENT: &str = "Environment";

/// A synthetic path label for `HKCU\Environment` [`PlatformError::Io`] errors
/// (the registry has no filesystem path).
fn hkcu_env_label() -> PathBuf {
    PathBuf::from(r"HKCU\Environment")
}

/// The current user's `HKCU\Environment\Path` value as a plain string, or `None`
/// when the value (or the `Environment` key) is absent. Read-only, unprivileged.
/// Consumed by the CLI's PATH management and the daemon's shim-dir-on-PATH doctor
/// probe (so `winreg` stays a single-crate dependency).
#[must_use]
pub fn user_path() -> Option<String> {
    user_path_raw().map(|(value, _)| value)
}

/// Read `HKCU\Environment\Path` as `(value, is_expand)`, where `is_expand` is
/// whether it is stored as `REG_EXPAND_SZ` (the conventional type, which must be
/// preserved on write so `%VAR%` references keep expanding). `None` when absent.
fn user_path_raw() -> Option<(String, bool)> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, REG_EXPAND_SZ};
    use winreg::types::FromRegValue;
    use winreg::RegKey;

    let env = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(HKCU_ENVIRONMENT, KEY_READ)
        .ok()?;
    let raw = env.get_raw_value("Path").ok()?;
    let is_expand = raw.vtype == REG_EXPAND_SZ;
    let value = String::from_reg_value(&raw).ok()?;
    Some((value, is_expand))
}

/// Write `HKCU\Environment\Path`, preserving the existing value type
/// (`REG_EXPAND_SZ` vs `REG_SZ`), or creating it as `REG_EXPAND_SZ` when absent.
///
/// Written through `winreg`'s raw value API rather than `setx.exe`, whose 1024-
/// character truncation of long `PATH`s is a data-loss bug. Unprivileged (the
/// user's own hive). Callers derive the new value from [`user_path`] and the pure
/// [`crate::pure::win_path_env`] editor.
pub fn set_user_path(value: &str) -> Result<(), PlatformError> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_WRITE, REG_EXPAND_SZ, REG_SZ};
    use winreg::{RegKey, RegValue};

    let expand = user_path_raw().map_or(true, |(_, is_expand)| is_expand);
    let env = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(HKCU_ENVIRONMENT, KEY_WRITE)
        .map_err(|source| PlatformError::Io {
            path: hkcu_env_label(),
            source,
        })?;
    let mut bytes: Vec<u8> = value.encode_utf16().flat_map(u16::to_le_bytes).collect();
    bytes.extend_from_slice(&[0, 0]);
    let vtype = if expand { REG_EXPAND_SZ } else { REG_SZ };
    env.set_raw_value("Path", &RegValue { bytes, vtype })
        .map_err(|source| PlatformError::Io {
            path: hkcu_env_label(),
            source,
        })
}

/// Broadcast an environment change to already-running processes (Explorer, and
/// so every terminal it launches) by setting a marker `HKCU\Environment` variable
/// `YERD_BIN` via `%SystemRoot%\System32\setx.exe`.
///
/// `setx` is used purely for its documented side effect: it always broadcasts
/// `WM_SETTINGCHANGE`, which is what makes a fresh shell pick up the new `PATH`
/// without a logoff. `PATH` itself is never written through `setx` (see
/// [`set_user_path`] for why); only this incidental, independently-useful marker
/// is. Best-effort: a non-zero exit is surfaced as [`PlatformError::Io`].
pub fn broadcast_user_env_marker(dir: &Path) -> Result<(), PlatformError> {
    let setx = win_system32("setx.exe");
    let status = std::process::Command::new(setx)
        .arg("YERD_BIN")
        .arg(dir)
        .output()
        .map_err(|source| PlatformError::Io {
            path: hkcu_env_label(),
            source,
        })?;
    if status.status.success() {
        Ok(())
    } else {
        Err(PlatformError::Io {
            path: hkcu_env_label(),
            source: std::io::Error::other(format!("setx exited with {}", status.status)),
        })
    }
}

/// Absolute path to a `System32` executable, from `%SystemRoot%` (falling back to
/// the conventional location), so a lookup never resolves an attacker-planted
/// binary on `PATH`.
fn win_system32(exe: &str) -> PathBuf {
    std::env::var_os("SystemRoot")
        .map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from)
        .join("System32")
        .join(exe)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use schannel::cert_store::Memory;

    use super::*;

    /// Mint a throwaway CA via `yerd-tls`; the returned PEM, DER, and
    /// fingerprint all describe the same certificate. Fingerprint identity is
    /// over the DER body, matching [`CaFingerprint::from_der`].
    fn mint_ca(cn: &str) -> (String, Vec<u8>, CaFingerprint) {
        let now = time::OffsetDateTime::now_utc();
        let v =
            yerd_tls::Validity::new(now - time::Duration::days(1), now + time::Duration::days(1))
                .unwrap();
        let ca = yerd_tls::CertAuthority::generate(cn, v).unwrap();
        let der = ca.cert_der().to_vec();
        let fp = CaFingerprint::from_der(&der);
        (ca.cert_pem().to_owned(), der, fp)
    }

    #[test]
    fn memory_store_add_find_delete_round_trip() {
        let (_pem, der, fp) = mint_ca("Yerd Memory Round-Trip CA");
        let mut store = Memory::new().unwrap().into_store();
        assert!(find_by_fp(&store, &fp).is_empty(), "absent before add");
        add_der(&mut store, &der).unwrap();
        let found = find_by_fp(&store, &fp);
        assert_eq!(found.len(), 1, "present after add");
        for cx in found {
            cx.delete().unwrap();
        }
        assert!(find_by_fp(&store, &fp).is_empty(), "absent after delete");
    }

    #[test]
    fn store_root_pem_renders_added_cert() {
        let (_pem, der, _fp) = mint_ca("Yerd Render CA");
        let mut store = Memory::new().unwrap().into_store();
        add_der(&mut store, &der).unwrap();
        let pem = store_root_pem(&store);
        assert!(pem.contains("BEGIN CERTIFICATE"), "{pem}");
    }

    /// The fingerprint integrity gate rejects a PEM whose DER does not match the
    /// expected fingerprint, and does so *before* any real store is opened (no
    /// dialog risk, CI-safe by construction).
    #[test]
    fn install_rejects_fingerprint_mismatch() {
        let (pem_a, _der_a, _fp_a) = mint_ca("Yerd CA A");
        let (_pem_b, _der_b, fp_b) = mint_ca("Yerd CA B");
        let err = WindowsTrustStore::new()
            .install_system(&pem_a, &fp_b)
            .unwrap_err();
        assert!(
            matches!(err, PlatformError::TrustStore { .. }),
            "expected a TrustStore error, got {err:?}"
        );
    }

    #[test]
    fn install_rejects_pem_without_certificate() {
        let fp = CaFingerprint::from_der(b"anything");
        let err = WindowsTrustStore::new()
            .install_system("not a pem", &fp)
            .unwrap_err();
        assert!(matches!(err, PlatformError::TrustStore { .. }), "{err:?}");
    }
}
