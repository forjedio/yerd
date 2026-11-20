//! `yerd daemon install|start|stop` - register and control the daemon service
//! for a headless install, without the desktop app.
//!
//! Local and daemon-free. Linux registers the same `yerd.service` systemd user
//! unit the desktop app uses (so `yerd-service-ctl` and `yerd uninstall` already
//! handle it) and asks for linger so it starts at boot. macOS has `yerd-helper`
//! write a root-owned system launchd daemon that runs `yerdd` as the invoking
//! user, so `install` (and `start` of that job) run under `sudo`. Removing the
//! registration is `yerd uninstall`'s job.
//!
//! The decisions are pure functions below; the `unix_impl` edge gathers the
//! facts (receipt, root, existing registrations, running daemon) and acts.

use std::path::Path;
use std::process::ExitCode;

use yerd_platform::pure::install_receipt::{InstallMode, ReceiptError};
use yerd_platform::pure::launchd_daemon::DaemonJobIdentity;

use crate::cli::DaemonAction;

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const EX_TEMPFAIL: u8 = 75;
const EX_USAGE: u8 = 64;
const EX_NOPERM: u8 = 77;
const EX_CONFIG: u8 = 78;

/// Run `yerd daemon <action>`. Returns the process exit code.
#[cfg(unix)]
pub fn run(action: DaemonAction) -> ExitCode {
    unix_impl::run(action)
}

/// Non-Unix: there is no headless service registration here.
#[cfg(not(unix))]
pub fn run(_action: DaemonAction) -> ExitCode {
    eprintln!("yerd: `yerd daemon` is only supported on macOS and Linux");
    ExitCode::from(EX_CONFIG)
}

/// Why a `yerd daemon` verb declined to act, and the exit code to use.
#[derive(Debug, PartialEq, Eq)]
struct Refusal {
    message: String,
    code: u8,
}

impl Refusal {
    fn new(message: impl Into<String>, code: u8) -> Self {
        Self {
            message: message.into(),
            code,
        }
    }
}

/// Only a headless install registers the daemon; the desktop app owns its own.
fn check_receipt(mode: Result<InstallMode, ReceiptError>) -> Result<(), Refusal> {
    match mode {
        Ok(InstallMode::Headless) => Ok(()),
        Ok(InstallMode::Package) => Err(Refusal::new(
            "this yerd belongs to the desktop app, which manages the daemon itself; \
             `yerd daemon install` is for headless installs",
            EX_CONFIG,
        )),
        Err(e) => Err(Refusal::new(e.to_string(), EX_CONFIG)),
    }
}

/// Where the Linux desktop packages put `yerdd` (directly, or as a symlink to
/// their `/usr/lib/<dir>` fallback).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const LINUX_DESKTOP_YERDD: &str = "/usr/bin/yerdd";

/// Whether systemd would rewrite `path` in `ExecStart=`: it splits on
/// whitespace, unquotes quotes and backslashes, and expands `%` specifiers and
/// `$` variables, so the unit would run some other path.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn systemd_unsafe_path(path: &Path) -> bool {
    path.to_string_lossy()
        .chars()
        .any(|c| c.is_whitespace() || matches!(c, '%' | '$' | '"' | '\'' | '\\'))
}

/// Linux `install`: refuse as root, refuse a `yerdd` path systemd can't run,
/// refuse next to a desktop app install, and refuse to overwrite a
/// `yerd.service` unit that runs a different `yerdd` (another install's
/// registration).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn linux_install_check(
    root: bool,
    desktop_app: bool,
    existing_unit: Option<&str>,
    unit_path: &Path,
    own_yerdd: &Path,
) -> Result<(), Refusal> {
    if root {
        return Err(Refusal::new(
            "run `yerd daemon install` as your own user, not with sudo: it registers a \
             systemd user service",
            EX_NOPERM,
        ));
    }
    if systemd_unsafe_path(own_yerdd) {
        return Err(Refusal::new(
            format!(
                "{} contains whitespace or one of % $ \" ' \\, which systemd's ExecStart= \
                 would rewrite; reinstall to a plain path (set YERD_INSTALL_DIR for the \
                 install script) and retry",
                own_yerdd.display()
            ),
            EX_CONFIG,
        ));
    }
    if desktop_app {
        return Err(Refusal::new(
            format!(
                "the desktop app is installed ({LINUX_DESKTOP_YERDD}) and manages the daemon; \
                 uninstall one of the two installs first"
            ),
            EX_CONFIG,
        ));
    }
    let Some(unit) = existing_unit else {
        return Ok(());
    };
    match yerd_service_ctl::unit_exec_start(unit) {
        Some(exec) if Path::new(exec) == own_yerdd => Ok(()),
        Some(exec) => Err(Refusal::new(
            format!(
                "{} already runs {exec}, from another Yerd install (the desktop app?); \
                 uninstall that install first",
                unit_path.display()
            ),
            EX_CONFIG,
        )),
        None => Err(Refusal::new(
            format!(
                "{} exists but was not written by Yerd; remove it and retry",
                unit_path.display()
            ),
            EX_CONFIG,
        )),
    }
}

/// Linux `install`, checked before the running daemon is stopped so a failure
/// leaves it up: a systemd user manager must be reachable and the unit
/// directory writable (`unit_writable` carries the probe's error).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn linux_register_preconditions(
    manager_reachable: bool,
    unit_writable: Result<(), String>,
) -> Result<(), Refusal> {
    if !manager_reachable {
        return Err(Refusal::new(
            "no systemd --user manager is reachable for this user, so the daemon can't be \
             registered; log in to a full session (or enable linger) and retry. The running \
             daemon was left alone",
            EX_CONFIG,
        ));
    }
    unit_writable
        .map_err(|e| Refusal::new(format!("{e}; the running daemon was left alone"), EX_CONFIG))
}

/// What macOS `install` saw before deciding.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Debug, Clone, Default)]
#[allow(clippy::struct_excessive_bools)]
struct MacInstallFacts {
    root: bool,
    sudo_uid: Option<u32>,
    /// The sudo user's account name.
    user: String,
    gui_app: bool,
    gui_agent: bool,
    system_plist: bool,
    /// Who the registered system job runs as, when its plist parses.
    registered: Option<DaemonJobIdentity>,
    daemon_running: bool,
}

/// macOS `install`: needs root via sudo (the job is a system launchd daemon),
/// refuses next to a desktop app install or its registration, and refuses
/// while a hand-started daemon is running. Returns the user to run as.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn macos_install_check(facts: &MacInstallFacts, yerd: &Path) -> Result<u32, Refusal> {
    if !facts.root {
        return Err(Refusal::new(
            format!(
                "registering the daemon to start at boot needs root. Run:\n  sudo {} daemon install",
                yerd.display()
            ),
            EX_NOPERM,
        ));
    }
    let Some(uid) = facts.sudo_uid else {
        return Err(Refusal::new(
            "run `yerd daemon install` through sudo from the account the daemon should run as",
            EX_NOPERM,
        ));
    };
    if facts.gui_app {
        return Err(Refusal::new(
            "the desktop app is installed (Yerd.app in /Applications or ~/Applications) and \
             manages the daemon; uninstall one of the two installs first",
            EX_CONFIG,
        ));
    }
    if facts.gui_agent {
        return Err(Refusal::new(
            "the desktop app's daemon is registered with launchd for this user; uninstall the \
             desktop app first",
            EX_CONFIG,
        ));
    }
    if let Some(job) = facts
        .registered
        .as_ref()
        .filter(|job| job.user != facts.user)
    {
        return Err(Refusal::new(
            format!(
                "the daemon job is registered for another account ({}, running {}); \
                 uninstall that install first",
                job.user, job.yerdd
            ),
            EX_CONFIG,
        ));
    }
    if !facts.system_plist && facts.daemon_running {
        return Err(Refusal::new(
            "a daemon you started by hand is running; run `yerd daemon stop` (as yourself, \
             without sudo) first",
            EX_CONFIG,
        ));
    }
    Ok(uid)
}

/// How macOS `start` brings the daemon up.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Debug, PartialEq, Eq)]
enum MacStart {
    /// As the user, through `yerd-service-ctl` (the desktop app's agent, or a
    /// `NeedsRoot` refusal for the system job).
    ServiceCtl,
    /// Under sudo, re-bootstrap the system job through the helper for `uid`.
    Helper(u32),
}

/// macOS `start` under sudo re-registers the system job for this install, so
/// it must already belong to this install and user; otherwise it would take
/// over another account's or install's job.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn registered_job_matches(
    registered: Option<&DaemonJobIdentity>,
    user: &str,
    yerdd: &Path,
) -> Result<(), Refusal> {
    match registered {
        Some(job) if job.user == user && Path::new(&job.yerdd) == yerdd => Ok(()),
        Some(job) => Err(Refusal::new(
            format!(
                "the registered daemon job runs {} as {}, not this install; start it with that \
                 install's yerd, or re-register with `sudo <this yerd> daemon install`",
                job.yerdd, job.user
            ),
            EX_CONFIG,
        )),
        None => Err(Refusal::new(
            "the registered daemon job's plist can't be read as one Yerd wrote; re-register \
             with `sudo <this yerd> daemon install`",
            EX_CONFIG,
        )),
    }
}

/// macOS `start`. A headless install without its system job has nothing for
/// `launchctl` to start, so it is pointed at `daemon install` instead.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn macos_start_plan(
    root: bool,
    sudo_uid: Option<u32>,
    system_plist: bool,
    headless: bool,
    yerd: &Path,
) -> Result<MacStart, Refusal> {
    let not_registered = || {
        Refusal::new(
            format!(
                "no headless daemon job is registered; run `sudo {} daemon install` first",
                yerd.display()
            ),
            EX_CONFIG,
        )
    };
    match (root, sudo_uid, system_plist) {
        (false, _, false) if headless => Err(not_registered()),
        (false, _, _) => Ok(MacStart::ServiceCtl),
        (true, Some(uid), true) => Ok(MacStart::Helper(uid)),
        (true, Some(_), false) => Err(not_registered()),
        (true, None, _) => Err(Refusal::new(
            "run `yerd daemon start` through sudo from the account the daemon runs as",
            EX_NOPERM,
        )),
    }
}

/// `start` on Linux and `stop` everywhere act on the user's own daemon, so
/// they refuse under sudo (where the uid would be root's).
fn as_user_check(root: bool, verb: &str) -> Result<(), Refusal> {
    if root {
        return Err(Refusal::new(
            format!(
                "run `yerd daemon {verb}` as yourself, without sudo: the daemon runs as your user"
            ),
            EX_USAGE,
        ));
    }
    Ok(())
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn needs_root_hint(yerd: &Path) -> String {
    format!(
        "the daemon is a system launchd job, so starting it needs root. Run:\n  sudo {} daemon start",
        yerd.display()
    )
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn linger_hint(user: &str) -> String {
    format!(
        "linger is off, so the daemon only runs while you are logged in. To start it at boot \
         with nobody logged in, run:\n  sudo loginctl enable-linger {user}"
    )
}

#[cfg(unix)]
mod unix_impl {
    use std::path::PathBuf;
    use std::process::ExitCode;

    use yerd_platform::install_mode;
    use yerd_platform::pure::install_receipt::ReceiptError;

    use super::{as_user_check, Refusal, EX_CONFIG};
    use crate::cli::DaemonAction;
    use crate::elevate;

    pub fn run(action: DaemonAction) -> ExitCode {
        let result = match action {
            DaemonAction::Install => install(),
            DaemonAction::Start => start(),
            DaemonAction::Stop => stop(),
        };
        match result {
            Ok(()) => ExitCode::SUCCESS,
            Err(refusal) => {
                eprintln!("yerd: {}", refusal.message);
                ExitCode::from(refusal.code)
            }
        }
    }

    fn own_dir() -> Result<PathBuf, Refusal> {
        install_mode::current_exe_dir()
            .ok_or_else(|| Refusal::new(ReceiptError::ExeUnresolved.to_string(), EX_CONFIG))
    }

    fn user_name(uid: u32) -> String {
        use nix::unistd::{Uid, User};
        match User::from_uid(Uid::from_raw(uid)) {
            Ok(Some(user)) => user.name,
            _ => std::env::var("USER").unwrap_or_else(|_| uid.to_string()),
        }
    }

    fn stop() -> Result<(), Refusal> {
        as_user_check(elevate::is_root(), "stop")?;
        let yerdd = install_mode::current_exe_dir()
            .unwrap_or_default()
            .join("yerdd");
        yerd_service_ctl::ServiceCtl::new(yerdd).stop();
        if os::wait_for_exit() {
            println!("yerd: daemon stopped");
            Ok(())
        } else {
            Err(Refusal::new(
                "the daemon is still running after the stop request",
                super::EX_TEMPFAIL,
            ))
        }
    }

    #[cfg(target_os = "linux")]
    fn install() -> Result<(), Refusal> {
        use yerd_service_ctl::LingerOutcome;

        super::check_receipt(install_mode::detect())?;
        let yerdd = own_dir()?.join("yerdd");
        let unit_path = yerd_service_ctl::systemd_unit_path().ok_or_else(|| {
            Refusal::new(
                "cannot resolve the systemd user unit directory ($HOME is unset)",
                EX_CONFIG,
            )
        })?;
        let existing = match std::fs::read_to_string(&unit_path) {
            Ok(content) => Some(content),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                return Err(Refusal::new(
                    format!("cannot read {}: {e}", unit_path.display()),
                    EX_CONFIG,
                ));
            }
        };
        let desktop_yerdd = std::path::Path::new(super::LINUX_DESKTOP_YERDD);
        let desktop_app = desktop_yerdd.exists() && yerdd != desktop_yerdd;
        super::linux_install_check(
            elevate::is_root(),
            desktop_app,
            existing.as_deref(),
            &unit_path,
            &yerdd,
        )?;
        if !yerdd.is_file() {
            return Err(Refusal::new(
                format!(
                    "{} is missing; reinstall with the install script",
                    yerdd.display()
                ),
                EX_CONFIG,
            ));
        }
        super::linux_register_preconditions(
            yerd_service_ctl::systemd_user_manager_reachable(),
            os::probe_unit_writable(&unit_path, existing.is_some()),
        )?;
        yerd_service_ctl::ServiceCtl::new(&yerdd).stop();
        if !yerd_service_ctl::wait_for_exit() {
            return Err(Refusal::new(
                "the old daemon did not exit; stop it and retry",
                super::EX_TEMPFAIL,
            ));
        }
        let linger = yerd_service_ctl::install_linux_unit(&yerdd)
            .map_err(|e| Refusal::new(format!("registering the daemon failed: {e}"), 1))?;
        println!(
            "yerd: registered the systemd user service `yerd` ({}) running {}",
            unit_path.display(),
            yerdd.display()
        );
        println!("yerd: the daemon is running");
        match linger {
            LingerOutcome::Enabled => {
                println!("yerd: linger is on, so it starts at boot without a login");
            }
            LingerOutcome::NotEnabled => {
                println!(
                    "yerd: {}",
                    super::linger_hint(&user_name(nix::unistd::getuid().as_raw()))
                );
            }
        }
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn start() -> Result<(), Refusal> {
        as_user_check(elevate::is_root(), "start")?;
        let yerdd = own_dir()?.join("yerdd");
        yerd_service_ctl::ServiceCtl::new(yerdd)
            .start()
            .map_err(|e| Refusal::new(format!("starting the daemon failed: {e}"), 1))?;
        println!("yerd: daemon started");
        Ok(())
    }

    #[cfg(target_os = "macos")]
    fn install() -> Result<(), Refusal> {
        use yerd_platform::pure::launchd_daemon::{DAEMON_LABEL, DAEMON_PLIST, GUI_AGENT_RELPATH};

        super::check_receipt(install_mode::detect())?;
        let yerd = own_dir()?.join("yerd");
        let sudo_uid = elevate::sudo_uid();
        let home = sudo_uid.and_then(os::home_of);
        let system_plist = std::path::Path::new(DAEMON_PLIST).exists();
        let facts = super::MacInstallFacts {
            root: elevate::is_root(),
            sudo_uid,
            user: sudo_uid.map(user_name).unwrap_or_default(),
            gui_app: std::path::Path::new("/Applications/Yerd.app").exists()
                || home
                    .as_ref()
                    .is_some_and(|h| h.join("Applications/Yerd.app").exists()),
            gui_agent: home
                .as_ref()
                .is_some_and(|h| h.join(GUI_AGENT_RELPATH).exists())
                || sudo_uid.is_some_and(os::gui_agent_loaded),
            system_plist,
            registered: os::registered_job(),
            daemon_running: !system_plist && sudo_uid.is_some_and(os::daemon_running_for),
        };
        let uid = super::macos_install_check(&facts, &yerd)?;
        os::run_install_daemon_job(uid)?;
        println!(
            "yerd: registered the launchd system job {DAEMON_LABEL} ({DAEMON_PLIST}); \
             the daemon runs as {} and starts at boot",
            user_name(uid)
        );
        Ok(())
    }

    #[cfg(target_os = "macos")]
    fn start() -> Result<(), Refusal> {
        use yerd_platform::pure::launchd_daemon::DAEMON_PLIST;

        let dir = own_dir()?;
        let yerd = dir.join("yerd");
        let plan = super::macos_start_plan(
            elevate::is_root(),
            elevate::sudo_uid(),
            std::path::Path::new(DAEMON_PLIST).exists(),
            matches!(
                install_mode::detect(),
                Ok(yerd_platform::pure::install_receipt::InstallMode::Headless)
            ),
            &yerd,
        )?;
        match plan {
            super::MacStart::ServiceCtl => yerd_service_ctl::ServiceCtl::new(dir.join("yerdd"))
                .start()
                .map_err(|e| match e {
                    yerd_service_ctl::ServiceError::NeedsRoot => {
                        Refusal::new(super::needs_root_hint(&yerd), super::EX_NOPERM)
                    }
                    other => Refusal::new(format!("starting the daemon failed: {other}"), 1),
                })?,
            super::MacStart::Helper(uid) => {
                super::registered_job_matches(
                    os::registered_job().as_ref(),
                    &user_name(uid),
                    &dir.join("yerdd"),
                )?;
                os::run_install_daemon_job(uid)?;
            }
        }
        println!("yerd: daemon started");
        Ok(())
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn install() -> Result<(), Refusal> {
        Err(Refusal::new(
            "`yerd daemon install` is only supported on macOS and Linux",
            EX_CONFIG,
        ))
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn start() -> Result<(), Refusal> {
        Err(Refusal::new(
            "`yerd daemon start` is only supported on macOS and Linux",
            EX_CONFIG,
        ))
    }

    #[cfg(target_os = "linux")]
    mod os {
        use std::path::Path;

        pub fn wait_for_exit() -> bool {
            yerd_service_ctl::wait_for_exit()
        }

        /// Whether the unit can be written, without changing it: create the
        /// unit dir, then open an existing unit for append or create and remove
        /// a probe file beside a missing one.
        pub fn probe_unit_writable(unit_path: &Path, exists: bool) -> Result<(), String> {
            let parent = unit_path
                .parent()
                .ok_or_else(|| format!("{} has no parent directory", unit_path.display()))?;
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
            if exists {
                return std::fs::OpenOptions::new()
                    .append(true)
                    .open(unit_path)
                    .map(drop)
                    .map_err(|e| format!("cannot write {}: {e}", unit_path.display()));
            }
            let probe = parent.join(format!(".yerd.service.probe-{}", std::process::id()));
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&probe)
                .map_err(|e| format!("cannot write to {}: {e}", parent.display()))?;
            let _ = std::fs::remove_file(&probe);
            Ok(())
        }
    }

    #[cfg(target_os = "macos")]
    mod os {
        use std::path::PathBuf;
        use std::process::Command;

        use yerd_platform::HelperInvocation;

        use crate::elevate;

        use super::Refusal;

        /// Bounded (~5s) wait for the current user's `yerdd` to exit.
        pub fn wait_for_exit() -> bool {
            let uid = nix::unistd::getuid().as_raw();
            for _ in 0..50 {
                if !daemon_running_for(uid) {
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            false
        }

        /// A `yerdd` owned by `uid` is running. Takes the uid explicitly
        /// because under sudo the process uid is root's.
        pub fn daemon_running_for(uid: u32) -> bool {
            Command::new("pgrep")
                .args(["-U", &uid.to_string(), "-x", "yerdd"])
                .output()
                .is_ok_and(|o| o.status.success())
        }

        /// The desktop app's per-user agent (`SMAppService` or the loose
        /// plist) is loaded in `uid`'s GUI domain.
        pub fn gui_agent_loaded(uid: u32) -> bool {
            let target = format!(
                "gui/{uid}/{}",
                yerd_platform::pure::launchd_daemon::DAEMON_LABEL
            );
            Command::new("launchctl")
                .args(["print", &target])
                .output()
                .is_ok_and(|o| o.status.success())
        }

        /// The registered system job's identity, if its plist exists and parses.
        pub fn registered_job() -> Option<yerd_platform::pure::launchd_daemon::DaemonJobIdentity> {
            let plist =
                std::fs::read_to_string(yerd_platform::pure::launchd_daemon::DAEMON_PLIST).ok()?;
            yerd_platform::pure::launchd_daemon::parse_daemon_plist(&plist)
        }

        pub fn home_of(uid: u32) -> Option<PathBuf> {
            use nix::unistd::{Uid, User};
            User::from_uid(Uid::from_raw(uid))
                .ok()
                .flatten()
                .map(|u| u.dir)
        }

        /// Have the sibling `yerd-helper` write and (re)load the system job.
        pub fn run_install_daemon_job(uid: u32) -> Result<(), Refusal> {
            let (helper, _yerdd) = elevate::sibling_binaries()
                .map_err(|e| Refusal::new(e.to_string(), super::EX_CONFIG))?;
            match elevate::spawn_helper(&helper, &HelperInvocation::InstallDaemonJob { uid }) {
                Ok(Some(0)) => Ok(()),
                Ok(Some(code)) => Err(Refusal::new(
                    format!("yerd-helper could not register the daemon job (exit {code})"),
                    u8::try_from(code).unwrap_or(1),
                )),
                Ok(None) => Err(Refusal::new("yerd-helper was killed by a signal", 1)),
                Err(e) => Err(Refusal::new(e.to_string(), 1)),
            }
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    mod os {
        pub fn wait_for_exit() -> bool {
            true
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    const YERD: &str = "/Users/me/.yerd/bin/yerd";

    #[test]
    fn receipt_gate_only_admits_headless() {
        assert_eq!(check_receipt(Ok(InstallMode::Headless)), Ok(()));
        let pkg = check_receipt(Ok(InstallMode::Package)).unwrap_err();
        assert!(pkg.message.contains("desktop app"), "{}", pkg.message);
        assert_eq!(pkg.code, EX_CONFIG);
        for err in [ReceiptError::Unrecognised, ReceiptError::ExeUnresolved] {
            let r = check_receipt(Err(err)).unwrap_err();
            assert_eq!(r.message, err.to_string());
            assert_eq!(r.code, EX_CONFIG);
        }
    }

    #[test]
    fn linux_install_check_table() {
        let unit_path = Path::new("/home/u/.config/systemd/user/yerd.service");
        let own = Path::new("/home/u/.yerd/bin/yerdd");
        let ours = yerd_service_ctl::render_systemd_unit(own);
        let gui = yerd_service_ctl::render_systemd_unit(Path::new("/usr/bin/yerdd"));
        let cases: [(bool, Option<&str>, Option<u8>); 6] = [
            (false, None, None),
            (false, Some(&ours), None),
            (false, Some(&gui), Some(EX_CONFIG)),
            (false, Some(""), Some(EX_CONFIG)),
            (true, None, Some(EX_NOPERM)),
            (true, Some(&ours), Some(EX_NOPERM)),
        ];
        for (root, existing, want) in cases {
            let got = linux_install_check(root, false, existing, unit_path, own)
                .err()
                .map(|r| r.code);
            assert_eq!(got, want, "root={root} existing={existing:?}");
        }
        let msg = linux_install_check(false, false, Some(&gui), unit_path, own)
            .unwrap_err()
            .message;
        assert!(msg.contains("/usr/bin/yerdd"), "{msg}");
    }

    #[test]
    fn linux_install_check_refuses_next_to_the_desktop_app() {
        let unit_path = Path::new("/home/u/.config/systemd/user/yerd.service");
        let own = Path::new("/home/u/.yerd/bin/yerdd");
        let r = linux_install_check(false, true, None, unit_path, own).unwrap_err();
        assert_eq!(r.code, EX_CONFIG);
        assert!(r.message.contains("desktop app"), "{}", r.message);
        assert_eq!(
            linux_install_check(true, true, None, unit_path, own)
                .unwrap_err()
                .code,
            EX_NOPERM
        );
    }

    #[test]
    fn linux_install_check_refuses_paths_systemd_would_rewrite() {
        let unit_path = Path::new("/home/u/.config/systemd/user/yerd.service");
        let cases = [
            ("/home/u/.yerd/bin/yerdd", None),
            ("/home/u/My Tools/bin/yerdd", Some(EX_CONFIG)),
            ("/home/u/tab\there/yerdd", Some(EX_CONFIG)),
            ("/opt/yerd\u{a0}bin/yerdd", Some(EX_CONFIG)),
            ("/home/u/yerd%h/bin/yerdd", Some(EX_CONFIG)),
            ("/home/u/$HOME/yerdd", Some(EX_CONFIG)),
            ("/home/u/it's/yerdd", Some(EX_CONFIG)),
            ("/home/u/a\"b/yerdd", Some(EX_CONFIG)),
            ("/home/u/a\\b/yerdd", Some(EX_CONFIG)),
            ("/home/u/yerd-1.0_x/yerdd", None),
        ];
        for (yerdd, want) in cases {
            let got = linux_install_check(false, false, None, unit_path, Path::new(yerdd))
                .err()
                .map(|r| r.code);
            assert_eq!(got, want, "{yerdd:?}");
        }
        let msg = linux_install_check(false, false, None, unit_path, Path::new("/a b/yerdd"))
            .unwrap_err()
            .message;
        assert!(msg.contains("YERD_INSTALL_DIR"), "{msg}");
        assert_eq!(
            linux_install_check(true, false, None, unit_path, Path::new("/a b/yerdd"))
                .unwrap_err()
                .code,
            EX_NOPERM
        );
    }

    #[test]
    fn linux_register_preconditions_table() {
        let cases: [(bool, Result<(), String>, Option<&str>); 4] = [
            (true, Ok(()), None),
            (false, Ok(()), Some("no systemd --user manager")),
            (
                true,
                Err("cannot create /x".to_owned()),
                Some("cannot create /x"),
            ),
            (
                false,
                Err("cannot create /x".to_owned()),
                Some("no systemd --user manager"),
            ),
        ];
        for (reachable, writable, needle) in cases {
            let got = linux_register_preconditions(reachable, writable.clone());
            match needle {
                None => assert_eq!(got, Ok(()), "reachable={reachable} {writable:?}"),
                Some(needle) => {
                    let r = got.unwrap_err();
                    assert_eq!(r.code, EX_CONFIG);
                    assert!(r.message.contains(needle), "{}", r.message);
                    assert!(r.message.contains("left alone"), "{}", r.message);
                }
            }
        }
    }

    #[test]
    fn macos_install_check_table() {
        let ok = MacInstallFacts {
            root: true,
            sudo_uid: Some(501),
            user: "me".to_owned(),
            ..MacInstallFacts::default()
        };
        let job = |user: &str| DaemonJobIdentity {
            yerdd: "/Users/x/.yerd/bin/yerdd".to_owned(),
            user: user.to_owned(),
        };
        let yerd = Path::new(YERD);
        assert_eq!(macos_install_check(&ok, yerd), Ok(501));
        assert_eq!(
            macos_install_check(
                &MacInstallFacts {
                    system_plist: true,
                    daemon_running: true,
                    registered: Some(job("me")),
                    ..ok.clone()
                },
                yerd
            ),
            Ok(501)
        );

        let refused = [
            (
                MacInstallFacts {
                    root: false,
                    ..ok.clone()
                },
                EX_NOPERM,
                "sudo /Users/me/.yerd/bin/yerd daemon install",
            ),
            (
                MacInstallFacts {
                    sudo_uid: None,
                    ..ok.clone()
                },
                EX_NOPERM,
                "through sudo",
            ),
            (
                MacInstallFacts {
                    gui_app: true,
                    ..ok.clone()
                },
                EX_CONFIG,
                "~/Applications",
            ),
            (
                MacInstallFacts {
                    gui_agent: true,
                    ..ok.clone()
                },
                EX_CONFIG,
                "registered with launchd",
            ),
            (
                MacInstallFacts {
                    system_plist: true,
                    registered: Some(job("alice")),
                    ..ok.clone()
                },
                EX_CONFIG,
                "another account (alice",
            ),
            (
                MacInstallFacts {
                    daemon_running: true,
                    ..ok.clone()
                },
                EX_CONFIG,
                "yerd daemon stop",
            ),
        ];
        for (facts, code, needle) in refused {
            let r = macos_install_check(&facts, yerd).unwrap_err();
            assert_eq!(r.code, code, "{facts:?}");
            assert!(r.message.contains(needle), "{facts:?}: {}", r.message);
        }
    }

    type StartCase = (bool, Option<u32>, bool, bool, Result<MacStart, u8>);

    #[test]
    fn macos_start_plan_table() {
        let yerd = Path::new(YERD);
        let cases: [StartCase; 10] = [
            (false, None, true, true, Ok(MacStart::ServiceCtl)),
            (false, None, true, false, Ok(MacStart::ServiceCtl)),
            (false, Some(501), false, false, Ok(MacStart::ServiceCtl)),
            (false, None, false, false, Ok(MacStart::ServiceCtl)),
            (false, None, false, true, Err(EX_CONFIG)),
            (false, Some(501), false, true, Err(EX_CONFIG)),
            (true, Some(501), true, true, Ok(MacStart::Helper(501))),
            (true, Some(501), false, true, Err(EX_CONFIG)),
            (true, None, true, true, Err(EX_NOPERM)),
            (true, None, false, false, Err(EX_NOPERM)),
        ];
        for (root, sudo_uid, plist, headless, want) in cases {
            let got = macos_start_plan(root, sudo_uid, plist, headless, yerd).map_err(|r| r.code);
            assert_eq!(
                got, want,
                "root={root} sudo_uid={sudo_uid:?} plist={plist} headless={headless}"
            );
        }
        for root in [false, true] {
            let r = macos_start_plan(root, Some(501), false, true, yerd).unwrap_err();
            assert_eq!(
                r.message,
                "no headless daemon job is registered; run \
                 `sudo /Users/me/.yerd/bin/yerd daemon install` first"
            );
        }
    }

    #[test]
    fn registered_job_matches_only_this_install_and_user() {
        let yerdd = Path::new("/Users/me/.yerd/bin/yerdd");
        let job = |path: &str, user: &str| DaemonJobIdentity {
            yerdd: path.to_owned(),
            user: user.to_owned(),
        };
        assert_eq!(
            registered_job_matches(Some(&job("/Users/me/.yerd/bin/yerdd", "me")), "me", yerdd),
            Ok(())
        );
        for (registered, needle) in [
            (
                Some(job("/Users/me/.yerd/bin/yerdd", "alice")),
                "as alice, not this install",
            ),
            (
                Some(job("/opt/yerd/yerdd", "me")),
                "runs /opt/yerd/yerdd as me",
            ),
            (None, "can't be read"),
        ] {
            let r = registered_job_matches(registered.as_ref(), "me", yerdd).unwrap_err();
            assert_eq!(r.code, EX_CONFIG);
            assert!(r.message.contains(needle), "{}", r.message);
        }
    }

    #[test]
    fn as_user_check_refuses_root() {
        assert_eq!(as_user_check(false, "stop"), Ok(()));
        let r = as_user_check(true, "stop").unwrap_err();
        assert_eq!(r.code, EX_USAGE);
        assert!(r.message.contains("without sudo"), "{}", r.message);
    }

    #[test]
    fn hints_carry_the_absolute_path_and_user() {
        assert!(
            needs_root_hint(Path::new(YERD)).contains("sudo /Users/me/.yerd/bin/yerd daemon start")
        );
        assert!(linger_hint("alice").contains("sudo loginctl enable-linger alice"));
    }
}
