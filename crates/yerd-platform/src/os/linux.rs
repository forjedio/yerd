//! Linux implementations of the host-platform traits.
//!
//! `Paths` uses XDG directories via the `directories` crate; the
//! `runtime` fallback parses `/proc/self/status` to find the real UID
//! when `XDG_RUNTIME_DIR` is unset. Privileged ops return
//! `NeedsHelper`; probes are read-only.

#![allow(clippy::similar_names)]

use std::fs;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use directories::ProjectDirs;

use crate::error::ops;
use crate::ide::IdeLauncher;
use crate::metrics::SystemMetrics;
use crate::opener::SystemOpener;
use crate::paths::{Paths, PlatformDirs};
use crate::port_binder::{BoundPort, PortBinder, PortPair};
use crate::port_redirect::PortRedirector;
use crate::pure::ide_spec::{desktop_entry_matches, spec_for};
use crate::pure::opener_spec::linux_default_openers;
use crate::pure::terminal_spec::{working_dir_flags, TERMINAL_SPECS};
use crate::pure::{
    networkmanager_dnsmasq, pem_match, port_plan, proc_metrics, resolved_drop_in, system_roots,
};
use crate::resolver::ResolverInstaller;
use crate::terminal::TerminalLauncher;
use crate::trust_store::{BrowserCaTrust, CaFingerprint, NssOutcome, TrustStore};
use crate::{
    BindPairErrorReason, Ide, IdeErrorReason, PlatformError, ResolverErrorReason,
    TerminalErrorReason, TrustStoreErrorReason,
};

/// Linux terminal launcher.
#[derive(Debug, Default, Clone, Copy)]
pub struct LinuxTerminalLauncher;

impl LinuxTerminalLauncher {
    /// Construct.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

/// The file name `program` actually runs, following symlinks. `x-terminal-emulator`
/// is Debian's alternatives link and can point at anything, so its flags have to
/// come from the link target: `gnome-terminal` needs `--working-directory`
/// because its D-Bus server does not inherit our `current_dir`, while handing
/// that same flag to `xterm` would stop it launching at all.
fn resolved_program(program: &str) -> String {
    let candidate = if program.contains('/') {
        Some(PathBuf::from(program))
    } else {
        std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|dir| dir.join(program))
                .find(|candidate| candidate.is_file())
        })
    };
    candidate
        .and_then(|path| fs::canonicalize(path).ok())
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| program.to_owned())
}

fn terminal_command(program: &str, path: &Path) -> Command {
    let mut command = Command::new(program);
    if let Some(flags) = working_dir_flags(&resolved_program(program)) {
        command.args(flags).arg(path);
    }
    command.current_dir(path);
    command
}

fn launch_terminal(program: &str, path: &Path) -> std::io::Result<()> {
    terminal_command(program, path).spawn().map(|_| ())
}

/// The terminal the user selected in Plasma, read from `kdeglobals`. Honoured
/// ahead of the probe list so a machine with both Kitty and Konsole installed
/// doesn't open the wrong one purely because of our probe order.
fn configured_kde_terminal() -> Option<String> {
    for reader in ["kreadconfig6", "kreadconfig5"] {
        let output = Command::new(reader)
            .args([
                "--file",
                "kdeglobals",
                "--group",
                "General",
                "--key",
                "TerminalApplication",
            ])
            .output()
            .ok();
        let Some(output) = output else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if !value.is_empty() {
            return Some(value);
        }
    }
    None
}

impl TerminalLauncher for LinuxTerminalLauncher {
    fn open_terminal(&self, path: &Path) -> Result<(), PlatformError> {
        let path_arg = path.to_string_lossy();
        if Command::new("xdg-terminal-exec")
            .arg(format!("--dir={path_arg}"))
            .current_dir(path)
            .spawn()
            .is_ok()
        {
            return Ok(());
        }
        if launch_terminal("x-terminal-emulator", path).is_ok() {
            return Ok(());
        }
        if let Some(program) = configured_kde_terminal() {
            if launch_terminal(&program, path).is_ok() {
                return Ok(());
            }
        }
        for (program, _) in TERMINAL_SPECS {
            if launch_terminal(program, path).is_ok() {
                return Ok(());
            }
        }
        Err(PlatformError::Terminal {
            reason: TerminalErrorReason::NoSupportedTerminal,
        })
    }
}

/// Linux IDE launcher using a command-line launcher or a freedesktop desktop entry.
#[derive(Debug, Default, Clone, Copy)]
pub struct LinuxIdeLauncher;

impl LinuxIdeLauncher {
    /// Construct.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

fn executable_in_directories<I>(name: &str, directories: I) -> Option<PathBuf>
where
    I: IntoIterator<Item = PathBuf>,
{
    directories
        .into_iter()
        .map(|directory| directory.join(name))
        .find(|candidate| {
            fs::metadata(candidate).is_ok_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            })
        })
}

fn executable_in_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")
        .and_then(|paths| executable_in_path_from_paths(name, std::env::split_paths(&paths)))
}

fn executable_in_path_from_paths<I>(name: &str, paths: I) -> Option<PathBuf>
where
    I: IntoIterator<Item = PathBuf>,
{
    executable_in_directories(name, paths)
}

fn ide_executable(ide: Ide) -> Option<PathBuf> {
    spec_for(ide).and_then(|spec| {
        spec.cli_names
            .iter()
            .find_map(|name| executable_in_path(name))
    })
}

/// Return the XDG application directories plus common package-manager exports.
fn application_dirs_from_parts(
    data_home: Option<&Path>,
    home: Option<&Path>,
    data_dirs: &[PathBuf],
) -> Vec<PathBuf> {
    let mut roots = Vec::new();

    let mut add_root = |root: PathBuf| {
        if !root.as_os_str().is_empty() && !roots.contains(&root) {
            roots.push(root);
        }
    };

    if let Some(data_home) = data_home {
        add_root(data_home.to_path_buf());
    }
    if let Some(home) = home {
        add_root(home.join(".local/share"));
        add_root(home.join(".local/share/flatpak/exports/share"));
        add_root(home.join(".nix-profile/share"));
    }

    for root in data_dirs {
        add_root(root.clone());
    }
    add_root(PathBuf::from("/var/lib/flatpak/exports/share"));
    add_root(PathBuf::from("/var/lib/snapd/desktop"));
    add_root(PathBuf::from("/run/current-system/sw/share"));
    roots
        .into_iter()
        .map(|root| root.join("applications"))
        .collect()
}

fn application_dirs() -> Vec<PathBuf> {
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let data_dirs = std::env::var_os("XDG_DATA_DIRS")
        .filter(|value| !value.is_empty())
        .map_or_else(
            || {
                vec![
                    PathBuf::from("/usr/local/share"),
                    PathBuf::from("/usr/share"),
                ]
            },
            |paths| std::env::split_paths(&paths).collect::<Vec<_>>(),
        );
    application_dirs_from_parts(data_home.as_deref(), home.as_deref(), &data_dirs)
}

fn desktop_entries_in(directory: &Path, depth: u8, matches: &mut Vec<(Ide, PathBuf)>) {
    if matches.len() == Ide::all().len() {
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_file()
            && path.extension().and_then(|extension| extension.to_str()) == Some("desktop")
        {
            let Ok(contents) = fs::read_to_string(&path) else {
                continue;
            };
            let file_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            for ide in Ide::all() {
                if matches.iter().any(|(found, _)| *found == *ide) {
                    continue;
                }
                if desktop_entry_matches(*ide, file_name, &contents) {
                    matches.push((*ide, path.clone()));
                    break;
                }
            }
        } else if file_type.is_dir() && depth > 0 {
            desktop_entries_in(&path, depth - 1, matches);
        }
        if matches.len() == Ide::all().len() {
            return;
        }
    }
}

fn desktop_entries_for_ides() -> Vec<(Ide, PathBuf)> {
    let mut matches = Vec::new();
    for directory in application_dirs() {
        desktop_entries_in(&directory, 1, &mut matches);
        if matches.len() == Ide::all().len() {
            break;
        }
    }
    matches
}

fn desktop_entry_for(ide: Ide) -> Option<PathBuf> {
    desktop_entries_for_ides()
        .into_iter()
        .find_map(|(found, path)| (found == ide).then_some(path))
}

fn launch_desktop_entry(desktop_entry: &Path, path: &Path) -> std::io::Result<()> {
    let mut last_error = None;
    for (program, subcommand, pass_path) in [
        ("gio", "launch", true),
        ("kioclient", "exec", false),
        ("kioclient5", "exec", false),
    ] {
        let mut command = Command::new(program);
        command.args([subcommand]).arg(desktop_entry);
        if pass_path {
            command.arg(path);
        }
        match wait_and_check(&mut command, program) {
            Ok(()) => return Ok(()),
            Err(source) => last_error = Some(source),
        }
    }
    Err(last_error.unwrap_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound)))
}

fn kde_session() -> bool {
    std::env::var_os("KDE_FULL_SESSION").is_some_and(|value| !value.is_empty())
        || std::env::var_os("XDG_CURRENT_DESKTOP").is_some_and(|value| {
            value
                .to_string_lossy()
                .split(':')
                .any(|desktop| desktop.eq_ignore_ascii_case("kde"))
        })
}

fn wait_and_check(command: &mut Command, program: &str) -> std::io::Result<()> {
    let output = command.output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "{program} exited with {}",
            output.status
        )))
    }
}

fn spawn_and_check(command: &mut Command, program: &str) -> std::io::Result<()> {
    let mut child = command.spawn()?;
    let deadline = Instant::now() + Duration::from_millis(250);
    loop {
        match child.try_wait()? {
            Some(status) if status.success() => return Ok(()),
            Some(status) => {
                return Err(std::io::Error::other(format!(
                    "{program} exited with {status}"
                )));
            }
            None if Instant::now() >= deadline => {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                return Ok(());
            }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

fn spawn_default_opener(program: &str, path: &Path) -> std::io::Result<()> {
    let mut command = Command::new(program);
    if program == "gio" {
        command.arg("open");
    }
    command.arg(path);
    wait_and_check(&mut command, program)
}

/// Linux system-default opener. KDE is checked first because some Plasma
/// sessions do not export `KDE_SESSION_VERSION`, which makes `xdg-open`
/// choose its obsolete `kfmclient` fallback instead of the native opener.
#[derive(Debug, Default, Clone, Copy)]
pub struct LinuxSystemOpener;

impl LinuxSystemOpener {
    /// Construct.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl SystemOpener for LinuxSystemOpener {
    fn open_path(&self, path: &Path) -> Result<(), PlatformError> {
        let mut last_error = None;
        for program in linux_default_openers(kde_session()) {
            match spawn_default_opener(program, path) {
                Ok(()) => return Ok(()),
                Err(source) => last_error = Some((program, source)),
            }
        }
        let Some((program, source)) = last_error else {
            return Err(PlatformError::SystemOpen {
                reason: crate::OpenErrorReason::NoSupportedOpener,
            });
        };
        Err(PlatformError::SystemOpen {
            reason: crate::OpenErrorReason::Launch {
                program: (*program).to_owned(),
                source,
            },
        })
    }
}

impl IdeLauncher for LinuxIdeLauncher {
    fn installed_ides(&self) -> Vec<Ide> {
        let desktop_entries = desktop_entries_for_ides();
        Ide::all()
            .iter()
            .copied()
            .filter(|ide| {
                ide_executable(*ide).is_some()
                    || desktop_entries.iter().any(|(found, _)| *found == *ide)
            })
            .collect()
    }

    fn open_in_ide(&self, ide: Ide, path: &Path) -> Result<(), PlatformError> {
        if let Some(executable) = ide_executable(ide) {
            let program = executable.to_string_lossy().into_owned();
            let mut command = Command::new(&executable);
            command.arg(path).current_dir(path);
            return match spawn_and_check(&mut command, &program) {
                Ok(()) => Ok(()),
                Err(source) => Err(PlatformError::Ide {
                    reason: IdeErrorReason::Launch { ide, source },
                }),
            };
        }

        if let Some(desktop_entry) = desktop_entry_for(ide) {
            return match launch_desktop_entry(&desktop_entry, path) {
                Ok(()) => Ok(()),
                Err(source) => Err(PlatformError::Ide {
                    reason: IdeErrorReason::Launch { ide, source },
                }),
            };
        }

        Err(PlatformError::Ide {
            reason: IdeErrorReason::NotInstalled(ide),
        })
    }
}

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

    fn install_firefox_nss(&self, ca_path: &Path) -> Result<NssOutcome, PlatformError> {
        Ok(crate::nss_exec::real_install(ca_path))
    }

    fn uninstall_firefox_nss(&self) -> Result<NssOutcome, PlatformError> {
        Ok(crate::nss_exec::real_uninstall())
    }

    fn browser_ca_trust(&self, fp: &CaFingerprint) -> Result<BrowserCaTrust, PlatformError> {
        Ok(crate::nss_exec::real_browser_trust(fp))
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

    fn is_installed(&self, tld: &str, addr: SocketAddr) -> Result<bool, PlatformError> {
        if tld.is_empty() {
            return Err(PlatformError::Resolver {
                reason: ResolverErrorReason::TldEmpty,
            });
        }

        let drop_in = drop_in_path(tld);
        if let Ok(text) = fs::read_to_string(drop_in) {
            if resolved_drop_in::parse(&text).is_some_and(|parsed| parsed.domain == tld) {
                return Ok(true);
            }
        }
        let nm = fs::read_to_string(networkmanager_path()).unwrap_or_default();
        let dnsmasq = fs::read_to_string(dnsmasq_path(tld)).unwrap_or_default();
        Ok(networkmanager_dnsmasq::matches_networkmanager(&nm)
            && networkmanager_dnsmasq::matches_dnsmasq(&dnsmasq, tld, addr))
    }
}

fn drop_in_path(tld: &str) -> PathBuf {
    PathBuf::from(format!("/etc/systemd/resolved.conf.d/yerd-{tld}.conf"))
}

fn networkmanager_path() -> PathBuf {
    PathBuf::from("/etc/NetworkManager/conf.d/yerd-dnsmasq.conf")
}

fn dnsmasq_path(tld: &str) -> PathBuf {
    PathBuf::from(format!("/etc/NetworkManager/dnsmasq.d/yerd-{tld}.conf"))
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

fn bind_at(ip: Ipv4Addr, port: u16) -> std::io::Result<TcpListener> {
    TcpListener::bind(SocketAddr::from((ip, port)))
}

fn bind_loopback(port: u16) -> std::io::Result<TcpListener> {
    bind_at(Ipv4Addr::LOCALHOST, port)
}

impl PortBinder for LinuxPortBinder {
    fn bind(&self, port: u16) -> Result<BoundPort, PlatformError> {
        bind_loopback(port)
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

pub(crate) fn bind_pair_impl(
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

    #[test]
    fn short_lived_process_status_is_reported() {
        let mut success = Command::new("/usr/bin/true");
        assert!(wait_and_check(&mut success, "/usr/bin/true").is_ok());

        let mut failure = Command::new("/usr/bin/false");
        assert!(wait_and_check(&mut failure, "/usr/bin/false").is_err());
    }

    #[test]
    fn direct_launcher_failure_is_reported_after_startup_check() {
        let mut command = Command::new("/usr/bin/false");
        assert!(spawn_and_check(&mut command, "/usr/bin/false").is_err());
    }

    #[test]
    fn direct_launcher_failure_after_initial_poll_is_reported() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 0.2; exit 7"]);
        assert!(spawn_and_check(&mut command, "/bin/sh").is_err());
    }

    #[test]
    fn executable_lookup_accepts_only_executable_files() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("zeditor");
        fs::write(&executable, b"#!/bin/sh\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(
            executable_in_directories("zeditor", vec![directory.path().to_path_buf()]),
            Some(executable)
        );
    }

    #[test]
    fn executable_path_lookup_uses_injected_path_entries() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("phpstorm");
        fs::write(&executable, b"#!/bin/sh\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(
            executable_in_path_from_paths("phpstorm", vec![directory.path().to_path_buf()]),
            Some(executable)
        );
    }

    #[test]
    fn desktop_entry_scan_finds_nested_application_entries() {
        let directory = tempfile::tempdir().unwrap();
        let nested = directory.path().join("applications");
        fs::create_dir(&nested).unwrap();
        let entry = nested.join("dev.zed.Zed.desktop");
        fs::write(
            &entry,
            "[Desktop Entry]\nType=Application\nName=Zed\nExec=zeditor %U\n",
        )
        .unwrap();

        let mut matches = Vec::new();
        desktop_entries_in(directory.path(), 1, &mut matches);
        assert_eq!(matches, vec![(Ide::Zed, entry)]);
    }

    #[test]
    fn application_dirs_include_injected_xdg_roots() {
        let directory = tempfile::tempdir().unwrap();
        let data_home = directory.path().join("data-home");
        let home = directory.path().join("home");
        let system_data = directory.path().join("system-data");
        let directories = application_dirs_from_parts(
            Some(&data_home),
            Some(&home),
            std::slice::from_ref(&system_data),
        );
        assert!(directories.contains(&data_home.join("applications")));
        assert!(directories.contains(&home.join(".local/share/applications")));
        assert!(directories.contains(&system_data.join("applications")));
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn bind_pair_impl_lan_binds_wildcard() {
        let pair = bind_pair_impl(true, (0, 0), (0, 0)).unwrap();
        assert_eq!(
            pair.http.listener.local_addr().unwrap().ip(),
            std::net::IpAddr::V4(Ipv4Addr::UNSPECIFIED)
        );
        assert_eq!(
            pair.https.listener.local_addr().unwrap().ip(),
            std::net::IpAddr::V4(Ipv4Addr::UNSPECIFIED)
        );
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn bind_pair_impl_lan_fallback_still_binds_wildcard() {
        let occupied = bind_at(Ipv4Addr::UNSPECIFIED, 0).unwrap();
        let taken = occupied.local_addr().unwrap().port();
        let pair = bind_pair_impl(true, (taken, 0), (0, 0)).unwrap();
        assert_eq!(
            pair.http.listener.local_addr().unwrap().ip(),
            std::net::IpAddr::V4(Ipv4Addr::UNSPECIFIED)
        );
        assert_eq!(
            pair.https.listener.local_addr().unwrap().ip(),
            std::net::IpAddr::V4(Ipv4Addr::UNSPECIFIED)
        );
        assert_ne!(pair.http.port().unwrap(), taken);
    }
}
