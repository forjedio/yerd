//! Windows OS implementation.
//!
//! Real `Windows*` types implement every trait in the crate: `Paths`,
//! `TrustStore`, `ResolverInstaller`, `PortBinder`, `PortRedirector`,
//! `TerminalLauncher`, `SystemOpener`, `SystemMetrics` and `IdeLauncher`. No
//! trait aliases the `os::unsupported` stub here any more, though that module
//! stays compiled on Windows so `cargo check --workspace` is green on any host.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use schannel::cert_context::CertContext;
use schannel::cert_store::{CertAdd, CertStore};

use crate::error::{
    ops, IdeErrorReason, OpenErrorReason, TerminalErrorReason, TrustStoreErrorReason,
};
use crate::ide::{DetectedIde, IdeLauncher, LaunchTarget};
use crate::metrics::SystemMetrics;
use crate::opener::SystemOpener;
use crate::paths::{Paths, PlatformDirs};
use crate::port_binder::{BoundPort, PortBinder, PortPair};
use crate::port_redirect::PortRedirector;
use crate::pure::ide_spec::{
    ide_cli_candidates_windows, spec_for, windows_executable_names, IdeSpec, IDE_SPECS,
};
use crate::pure::{
    nrpt, pem_match, win_metrics, win_pipe, win_port_owner, win_terminal, win_token,
};
use crate::resolver::ResolverInstaller;
use crate::terminal::TerminalLauncher;
use crate::trust_store::{CaFingerprint, NssOutcome, TrustStore};
use crate::PlatformError;

/// `CREATE_NEW_CONSOLE` process-creation flag: the spawned shell gets its own
/// console window instead of inheriting the caller's (the daemon/GUI has none
/// worth sharing). Safe std `creation_flags`, no FFI.
const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

/// `CREATE_NO_WINDOW` process-creation flag: a console child runs with no
/// window at all, so a console-less parent (the daemon) never flashes one.
/// Safe std `creation_flags`, no FFI.
///
/// The canonical definition for the whole workspace. Prefer [`hidden_command`],
/// which applies it for you.
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// `CREATE_NEW_PROCESS_GROUP` process-creation flag: the child leads its own
/// process group, so a console Ctrl-C aimed at the parent does not also reach
/// it. Safe std `creation_flags`, no FFI.
///
/// The canonical definition for the whole workspace.
pub const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

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
/// fallback retry as Linux, attempting the desired ports directly.
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsPortBinder;

impl WindowsPortBinder {
    /// Construct.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl PortBinder for WindowsPortBinder {
    fn bind(&self, port: u16) -> Result<BoundPort, PlatformError> {
        super::port_bind::bind_at(Ipv4Addr::LOCALHOST, port)
            .map(|listener| BoundPort { listener })
            .map_err(|source| PlatformError::Bind { port, source })
    }

    fn bind_pair(
        &self,
        lan: bool,
        desired: (u16, u16),
        fallback: (u16, u16),
    ) -> Result<PortPair, PlatformError> {
        super::port_bind::bind_pair_impl(false, lan, desired, fallback)
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

/// The DNS servers every NRPT rule for `.tld` forwards to, flattened across
/// rules in registry enumeration order.
///
/// The `is_installed` probe collapses to a bare `bool`, so it cannot tell "no
/// rule at all" from "a rule pointing somewhere else". This reports the actual
/// targets so the doctor can name them. Empty means no `.tld` rule exists (or it
/// carries no servers). Read-only, unprivileged.
#[must_use]
pub fn nrpt_servers_for_tld(tld: &str) -> Vec<String> {
    read_nrpt_rules()
        .into_iter()
        .filter(|rule| nrpt::name_matches_tld(&rule.name, tld))
        .flat_map(|rule| nrpt::split_servers(&rule.servers))
        .collect()
}

/// The image name of the process holding UDP `port` on loopback, e.g.
/// `"dnscrypt-proxy.exe"`.
///
/// Spawns `%SystemRoot%\System32\netstat.exe -a -n -o -p UDP` and then
/// `tasklist.exe` for the PID it finds (absolute paths, never `PATH`), and
/// parses both with the table-tested [`win_port_owner`] parsers. Both the
/// loopback and the wildcard local address are checked: a squatter bound to
/// `0.0.0.0:<port>` blocks a `127.0.0.1:<port>` bind just as surely as one bound
/// to loopback, and is the more common shape. `None` when either tool fails, no
/// row matches, or the PID has already exited.
///
/// UDP only. `yerd_dns::Bound::bind` binds UDP and TCP, so a TCP-only squatter
/// goes unnamed; the caller degrades to the portless message it had before.
/// No `unsafe`: the `GetExtendedUdpTable` alternative is FFI this crate's
/// `#![forbid(unsafe_code)]` rules out, matching the existing `whoami.exe`
/// precedent.
#[must_use]
pub fn udp_port_owner(port: u16) -> Option<String> {
    let netstat = run_console_tool("netstat.exe", &["-a", "-n", "-o", "-p", "UDP"])?;
    let pid = [format!("127.0.0.1:{port}"), format!("0.0.0.0:{port}")]
        .iter()
        .find_map(|addr| win_port_owner::udp_owning_pid(&netstat, addr))?;
    let filter = format!("PID eq {pid}");
    let csv = run_console_tool("tasklist.exe", &["/FI", &filter, "/FO", "CSV", "/NH"])?;
    win_port_owner::image_name(&csv)
}

/// Run a `System32` console tool and return its stdout, preserving the failure.
///
/// Spawned through [`hidden_command`], so a console-less parent never flashes a
/// window. A non-zero exit becomes an [`std::io::Error`] so callers that must
/// report *why* the tool failed can do so; [`run_console_tool`] is the
/// discarding form.
fn try_console_tool(exe: &str, args: &[&str]) -> Result<String, std::io::Error> {
    let output = hidden_command(&system32_exe(exe)).args(args).output()?;
    if !output.status.success() {
        return Err(std::io::Error::other(format!(
            "exited with {}",
            output.status
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Run a `System32` console tool and return its stdout, or `None` on any
/// spawn/exit failure. `CREATE_NO_WINDOW` keeps the daemon (which has no console
/// of its own) from flashing one up on every status poll.
fn run_console_tool(exe: &str, args: &[&str]) -> Option<String> {
    try_console_tool(exe, args).ok()
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
    run_console_tool("whoami.exe", &["/groups", "/fo", "csv", "/nh"])
        .is_some_and(|stdout| win_token::csv_has_elevated_integrity(&stdout))
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
///
/// Adding to a real "Root" store raises the Windows confirmation dialog, but a
/// success here does not mean the certificate was written: the same call reports
/// success when the user declines the dialog and when there is no interactive
/// desktop to show it on. Callers that add to a real store must therefore verify
/// the result themselves rather than trust the returned `Ok`.
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
/// must run in the CLI or GUI process and NEVER in `yerdd`, which runs as a
/// session-0 service with no desktop.
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
    /// Install `ca_pem` into the `CurrentUser` Root store, and verify that it
    /// landed.
    ///
    /// The fingerprint is verified against the exact DER bytes about to be
    /// imported (the integrity gate the macOS helper flow uses) before any store
    /// is opened, so a mismatch fails without side effects.
    ///
    /// The add itself pops the Windows root-store confirmation dialog, but its
    /// success is not evidence of anything: the Win32 add reports success even
    /// when nothing was written, both when the user declines the dialog and when
    /// there is no interactive desktop to show the dialog on. The write is
    /// therefore confirmed by re-probing [`Self::is_present_system`], and only a
    /// certificate actually found in the store counts as an install.
    ///
    /// The write handle is closed before that probe opens its own, which is
    /// load-bearing rather than tidiness: crypt32 keeps a system store's cached
    /// data alive for as long as any handle to it is open, and `add_cert`
    /// mutates that in-memory cache, so a probe made through a store opened
    /// while the write handle is still live is exactly the arrangement in which
    /// a silently dropped add stays visible and the check verifies nothing.
    ///
    /// # Errors
    ///
    /// An error after a successful add means the certificate was not installed,
    /// either because the confirmation dialog was declined or because this
    /// process had no interactive desktop to show it on. The two are
    /// deliberately not distinguished: Win32 returns the same success from the
    /// same call in both cases, so telling them apart would need a
    /// session-detection layer this crate does not have. The returned message
    /// names both causes instead.
    ///
    /// Must run in the CLI or GUI, never from the daemon, which has no desktop.
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
        {
            let mut store = CertStore::open_current_user("Root").map_err(sys_api)?;
            add_der(&mut store, &der)?;
        }
        if self.is_present_system(fp)? {
            Ok(())
        } else {
            Err(PlatformError::TrustStore {
                reason: TrustStoreErrorReason::SystemApi(
                    "the certificate was not added to the CurrentUser Root store: the \
                     confirmation dialog was declined, or this process has no interactive \
                     desktop to show it on"
                        .to_owned(),
                ),
            })
        }
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

    /// On Windows, presence in the Root store *is* trust: there is no separate
    /// trust-settings layer as on macOS, so the effective-trust probe is the same
    /// as the presence probe and `ca_path` is unused.
    fn is_trusted(&self, _ca_path: &Path, fp: &CaFingerprint) -> Result<bool, PlatformError> {
        self.is_present_system(fp)
    }

    /// Windows needs no NSS path, so this stays `Unsupported` deliberately.
    /// Every supported browser follows the `CurrentUser\Root` store that
    /// `install_system` writes: Chromium-family reads it natively, and Firefox
    /// imports it through `security.enterprise_roots.enabled`, which ships
    /// `true` by default (measured on stock release 153.0.3, no policy in
    /// force). The certutil/NSS route is a closed decision, not pending work.
    ///
    /// Two observations that mislead anyone re-checking this. `about:certificate`
    /// does not list the CA even while Firefox trusts it, because roots imported
    /// from the OS never enter the NSS database that tab enumerates; check the
    /// padlock's "Verified by" line or the Windows store instead. And Firefox
    /// snapshots the root store at startup, so adding or removing the CA only
    /// reaches Firefox after a restart.
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

/// The current user's SID, via `whoami /user`, parsed by the table-tested
/// [`win_pipe`] parser. Spawned through [`try_console_tool`], so it resolves an
/// absolute `System32` path and never flashes a console window.
fn spawn_whoami_sid() -> Result<String, PlatformError> {
    let stdout = try_console_tool("whoami.exe", &["/user", "/fo", "csv", "/nh"]).map_err(|e| {
        PlatformError::SidLookup {
            detail: format!("whoami {e}"),
        }
    })?;
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

/// The current user's `HKCU\Environment\Path` value as a plain string.
/// Read-only, unprivileged. Consumed by the CLI's PATH management and the
/// daemon's shim-dir-on-PATH doctor probe (so `winreg` stays a single-crate
/// dependency).
///
/// # Errors
///
/// `Ok(None)` is an absent value (or absent `Environment` key); [`Err`] is a
/// failed read. Callers writing an edited value back **must** distinguish them -
/// see [`user_path_raw`].
pub fn user_path() -> Result<Option<String>, PlatformError> {
    user_path_raw().map(|opt| opt.map(|(value, _)| value))
}

/// Read `HKCU\Environment\Path` as `(value, is_expand)`, where `is_expand` is
/// whether it is stored as `REG_EXPAND_SZ` (the conventional type, which must be
/// preserved on write so `%VAR%` references keep expanding).
///
/// `Ok(None)` means the value is genuinely **absent** (a fresh profile), for
/// which an empty PATH is the correct basis of an edit. `Err` means the read
/// itself failed - a different thing entirely, and one a caller that writes a
/// derived value back must not treat as "empty", or an unreadable-but-present
/// PATH gets replaced by whatever was derived from nothing. Keeping the two
/// apart is the whole point of the `Result<Option<_>>`.
fn user_path_raw() -> Result<Option<(String, bool)>, PlatformError> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, REG_EXPAND_SZ};
    use winreg::types::FromRegValue;
    use winreg::RegKey;

    let io_err = |source: std::io::Error| PlatformError::Io {
        path: hkcu_env_label(),
        source,
    };
    let env = match RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(HKCU_ENVIRONMENT, KEY_READ)
    {
        Ok(key) => key,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io_err(e)),
    };
    let raw = match env.get_raw_value("Path") {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io_err(e)),
    };
    let is_expand = raw.vtype == REG_EXPAND_SZ;
    let value = String::from_reg_value(&raw).map_err(io_err)?;
    Ok(Some((value, is_expand)))
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

    let expand = match user_path_raw()? {
        Some((_, is_expand)) => is_expand,
        None => true,
    };
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
    let setx = system32_exe("setx.exe");
    let status = hidden_command(&setx)
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

/// The Windows installation root, from `%SystemRoot%`, falling back to the
/// conventional location when the variable is unset.
///
/// The canonical definition for the whole workspace.
#[must_use]
pub fn system_root() -> PathBuf {
    std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from)
}

/// Absolute path to a `System32` executable, derived from [`system_root`] so a
/// lookup never resolves an attacker-planted binary on `PATH`.
///
/// The canonical definition for the whole workspace.
#[must_use]
pub fn system32_exe(exe: &str) -> PathBuf {
    system_root().join("System32").join(exe)
}

/// Absolute path to `explorer.exe`, which lives directly under the Windows
/// installation root rather than in `System32`.
#[must_use]
fn explorer_exe() -> PathBuf {
    system_root().join("explorer.exe")
}

/// A [`Command`] for `exe` that will not flash a console window, for a parent
/// that has no console of its own.
///
/// The canonical way to spawn a console child anywhere in the workspace: it
/// applies [`CREATE_NO_WINDOW`] so no caller has to remember the flag.
#[must_use]
pub fn hidden_command(exe: &Path) -> std::process::Command {
    use std::os::windows::process::CommandExt as _;

    let mut cmd = std::process::Command::new(exe);
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

/// First existing file among `windows_executable_names(name)` across `dirs`.
///
/// Windows has no execute bit, so a plain file test is the whole check; the
/// Unix twin in `os::unix` additionally tests the mode.
fn executable_in_directories<I>(name: &str, dirs: I) -> Option<PathBuf>
where
    I: IntoIterator<Item = PathBuf>,
{
    let names = windows_executable_names(name);
    dirs.into_iter().find_map(|dir| {
        names.iter().find_map(|file| {
            let candidate = dir.join(file);
            std::fs::metadata(&candidate)
                .ok()
                .filter(std::fs::Metadata::is_file)
                .map(|_| candidate)
        })
    })
}

/// `PATH` first, then the `JetBrains` Toolbox scripts directory.
fn executable_in_path(name: &str) -> Option<PathBuf> {
    if let Some(paths) = std::env::var_os("PATH") {
        if let Some(exe) = executable_in_directories(name, std::env::split_paths(&paths)) {
            return Some(exe);
        }
    }
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    executable_in_directories(name, ide_cli_candidates_windows(local.as_deref()))
}

fn ide_executable(spec: &IdeSpec) -> Option<PathBuf> {
    spec.cli_names
        .iter()
        .find_map(|name| executable_in_path(name))
}

/// Real `IdeLauncher` for Windows.
///
/// Detection is a handful of `metadata` probes over `PATH` plus the Toolbox
/// scripts directory, so unlike the Unix adapters there is no second
/// application-scan pass: there is nothing an `/Applications` walk or an XDG
/// desktop-entry scan would add. Every hit is therefore a
/// [`LaunchTarget::Cli`].
///
/// A `.cmd` or `.bat` shim is run by `std`, which detects the batch extension
/// and re-targets `cmd.exe` itself, owning the argument escaping. Success is a
/// successful spawn: the startup-window check the Unix adapters use guards
/// against a broken `PATH` script, which the direct file probe here already
/// rules out.
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsIdeLauncher;

impl WindowsIdeLauncher {
    /// Construct.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl IdeLauncher for WindowsIdeLauncher {
    fn detect(&self) -> Vec<DetectedIde> {
        let mut found: Vec<DetectedIde> = IDE_SPECS
            .iter()
            .filter_map(|spec| {
                ide_executable(spec).map(|exe| DetectedIde {
                    id: spec.id,
                    display_name: spec.display_name,
                    launch: LaunchTarget::Cli(exe),
                })
            })
            .collect();
        found.sort_by_key(|ide| spec_for(ide.id).map_or(u8::MAX, |spec| spec.rank));
        found
    }

    fn launch(&self, ide: &DetectedIde, path: &Path) -> Result<(), PlatformError> {
        let (LaunchTarget::Cli(exe) | LaunchTarget::Application(exe)) = &ide.launch;
        hidden_command(exe)
            .arg(path)
            .current_dir(path)
            .spawn()
            .map(|_| ())
            .map_err(|source| PlatformError::Ide {
                reason: IdeErrorReason::Launch {
                    ide: ide.display_name.to_owned(),
                    source,
                },
            })
    }
}

/// Real `SystemOpener` for Windows.
///
/// Hands the path to `explorer.exe`, the shell's own handler, which selects the
/// right action for a directory or a file. A successful **spawn** is the success
/// signal: Explorer routes the request to the already-running shell process and
/// then exits non-zero, so inspecting its status would misreport every
/// successful open as a failure. The child is not waited on for the same reason.
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsSystemOpener;

impl WindowsSystemOpener {
    /// Construct.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl SystemOpener for WindowsSystemOpener {
    fn open_path(&self, path: &Path) -> Result<(), PlatformError> {
        hidden_command(&explorer_exe())
            .arg(path)
            .spawn()
            .map(|_| ())
            .map_err(|source| PlatformError::SystemOpen {
                reason: OpenErrorReason::Launch {
                    program: "explorer.exe".to_owned(),
                    source,
                },
            })
    }
}

/// Real `SystemMetrics` for Windows.
///
/// `rss_bytes` reports the process working set, via `tasklist.exe`, which is the
/// closest Windows analogue of Unix RSS: both count resident physical pages.
/// Every failure to spawn, exit or parse collapses to `None`, because metrics
/// are best-effort decoration and must never fail a status call.
///
/// `load_average` is `None`: Windows has no load-average concept, the same
/// answer the macOS impl gives for the same reason.
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
    fn rss_bytes(&self, pid: u32) -> Option<u64> {
        let filter = format!("PID eq {pid}");
        let csv = run_console_tool("tasklist.exe", &["/FI", &filter, "/FO", "CSV", "/NH"])?;
        win_metrics::parse_tasklist_mem_bytes(&csv)
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
    use schannel::cert_store::Memory;

    use super::*;

    /// `explorer.exe` lives directly under the Windows root, not in `System32`.
    /// Composing it through [`system32_exe`] would make every "reveal in folder"
    /// spawn a path that does not exist.
    #[test]
    fn explorer_exe_sits_at_the_windows_root_not_system32() {
        let p = explorer_exe();
        assert!(p.is_absolute(), "{p:?}");
        assert!(p.ends_with("explorer.exe"), "{p:?}");
        assert!(!p.starts_with(system_root().join("System32")), "{p:?}");
        assert_eq!(p.parent(), Some(system_root().as_path()));
    }

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
