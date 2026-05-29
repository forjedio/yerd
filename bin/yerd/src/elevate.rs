//! `yerd elevate` / `yerd unelevate` — one-shot privileged setup, run via sudo.
//!
//! The CLI runs as root **only to orchestrate**: it fetches read-only facts
//! from the invoking user's running daemon (over that user's socket, located
//! from `SUDO_UID`), then spawns the audited `yerd-helper` for each privileged
//! operation. The helper independently re-validates every argument; this module
//! additionally (a) derives the `yerdd` binary from its own trusted
//! `current_exe` sibling — never from the daemon — and (b) owner-checks the CA
//! path before trusting it. The daemon itself is never restarted as root.

#[cfg(not(unix))]
pub async fn run_elevate(
    _target: Option<crate::cli::ElevateTarget>,
    _undo: bool,
) -> std::process::ExitCode {
    eprintln!("yerd: elevate is only supported on Unix (macOS/Linux)");
    std::process::ExitCode::from(78)
}

#[cfg(unix)]
pub use unix_impl::run_elevate;

#[cfg(unix)]
mod unix_impl {
    use std::net::SocketAddr;
    use std::path::{Path, PathBuf};
    use std::process::{Command, ExitCode};

    use yerd_ipc::{Request, Response};
    use yerd_platform::{CaFingerprint, HelperInvocation};

    use crate::cli::ElevateTarget;
    use crate::error::ClientError;
    use crate::transport;

    /// Read-only daemon facts needed to drive the helper.
    struct Facts {
        dns_addr: SocketAddr,
        tld: String,
        ca_path: PathBuf,
        ca_fingerprint: String,
    }

    /// Expand an optional target into the concrete list (None = all, in
    /// trust → resolver → ports order).
    fn targets(target: Option<ElevateTarget>) -> Vec<ElevateTarget> {
        match target {
            Some(t) => vec![t],
            None => vec![
                ElevateTarget::Trust,
                ElevateTarget::Resolver,
                ElevateTarget::Ports,
            ],
        }
    }

    /// Entry point. Returns the process exit code; prints progress/errors.
    pub async fn run_elevate(target: Option<ElevateTarget>, undo: bool) -> ExitCode {
        if !is_root() {
            eprintln!("yerd: elevate must run as root — try: sudo yerd elevate");
            return ExitCode::from(77);
        }

        let facts = match fetch_facts().await {
            Ok(f) => f,
            Err(e) => {
                eprintln!("yerd: {e}");
                return ExitCode::from(69);
            }
        };

        let (helper, yerdd) = match sibling_binaries() {
            Ok(pair) => pair,
            Err(e) => {
                eprintln!("yerd: {e}");
                return ExitCode::from(74);
            }
        };

        let mut any_failed = false;
        for t in targets(target) {
            if let Err(e) = run_one(t, &facts, &helper, &yerdd, undo) {
                eprintln!("    failed: {e}");
                any_failed = true;
            }
        }
        if any_failed {
            ExitCode::from(1)
        } else {
            ExitCode::SUCCESS
        }
    }

    /// Run a single target: build the invocation, spawn the helper (or print
    /// guidance), and classify the outcome by exit code.
    fn run_one(
        target: ElevateTarget,
        facts: &Facts,
        helper: &Path,
        yerdd: &Path,
        undo: bool,
    ) -> Result<(), ClientError> {
        // Best-effort revert of `ports`: setcap has no clean reverse op.
        if target == ElevateTarget::Ports && undo {
            println!("==> ports: capabilities can't be dropped automatically.");
            println!(
                "    run manually if desired: sudo setcap -r {}",
                yerdd.display()
            );
            return Ok(());
        }

        // The CA pem is the only path we take from the daemon; verify it's owned
        // by the invoking user and not world-writable before trusting it.
        if target == ElevateTarget::Trust && !undo {
            require_user_owned(&facts.ca_path, invoking_uid())?;
        }

        let inv = plan_invocation(target, facts, yerdd, undo)?;
        println!("==> {}", describe(target, undo, facts));

        match spawn_helper(helper, &inv)? {
            Some(0) => {
                println!("    ok");
                if target == ElevateTarget::Ports && !undo {
                    println!(
                        "    restart the yerd daemon (as your user) for 80/443 to take effect."
                    );
                    println!(
                        "    note: package upgrades reset setcap — re-run `elevate ports` then."
                    );
                }
                Ok(())
            }
            // EX_CONFIG (78): the helper deems this unsupported on this host
            // (e.g. resolver without systemd-resolved). A skip, not a failure.
            Some(78) => {
                println!("    skipped (unsupported on this host)");
                if target == ElevateTarget::Resolver {
                    println!(
                        "    no systemd-resolved — configure /etc/resolv.conf for {} manually.",
                        facts.dns_addr
                    );
                }
                Ok(())
            }
            Some(code) => Err(ClientError::Usage(format!(
                "yerd-helper exited with status {code}"
            ))),
            None => Err(ClientError::Usage(
                "yerd-helper was terminated by a signal".to_owned(),
            )),
        }
    }

    /// Pure: map a target to the helper invocation. Never called for
    /// `ports`+undo (`run_one` short-circuits that as guidance).
    fn plan_invocation(
        target: ElevateTarget,
        facts: &Facts,
        yerdd: &Path,
        undo: bool,
    ) -> Result<HelperInvocation, ClientError> {
        let fp =
            || CaFingerprint::from_hex(&facts.ca_fingerprint).map_err(ClientError::Fingerprint);
        Ok(match (target, undo) {
            (ElevateTarget::Trust, false) => HelperInvocation::InstallCa {
                ca_pem_path: facts.ca_path.clone(),
                fp: fp()?,
            },
            (ElevateTarget::Trust, true) => HelperInvocation::UninstallCa { fp: fp()? },
            (ElevateTarget::Resolver, false) => HelperInvocation::InstallResolver {
                tld: facts.tld.clone(),
                addr: facts.dns_addr,
            },
            (ElevateTarget::Resolver, true) => HelperInvocation::UninstallResolver {
                tld: facts.tld.clone(),
            },
            (ElevateTarget::Ports, false) => HelperInvocation::Setcap {
                daemon_binary: yerdd.to_path_buf(),
            },
            (ElevateTarget::Ports, true) => {
                return Err(ClientError::Usage("ports cannot be reverted".to_owned()))
            }
        })
    }

    fn describe(target: ElevateTarget, undo: bool, facts: &Facts) -> String {
        match (target, undo) {
            (ElevateTarget::Trust, false) => {
                "trust: trusting the local CA in the system store".into()
            }
            (ElevateTarget::Trust, true) => {
                "trust: removing the local CA from the system store".into()
            }
            (ElevateTarget::Resolver, false) => {
                format!("resolver: routing *.{} → {}", facts.tld, facts.dns_addr)
            }
            (ElevateTarget::Resolver, true) => {
                format!("resolver: removing the *.{} route", facts.tld)
            }
            (ElevateTarget::Ports, false) => "ports: granting cap_net_bind_service to yerdd".into(),
            (ElevateTarget::Ports, true) => "ports: (no-op)".into(),
        }
    }

    /// Connect to the invoking user's daemon socket and fetch `DaemonInfo`.
    async fn fetch_facts() -> Result<Facts, ClientError> {
        let mut last_err: Option<ClientError> = None;
        for sock in socket_candidates() {
            match transport::exchange_at(&sock, &Request::DaemonInfo).await {
                Ok(Response::Info {
                    dns_addr,
                    tld,
                    ca_path,
                    ca_fingerprint,
                }) => {
                    return Ok(Facts {
                        dns_addr,
                        tld,
                        ca_path,
                        ca_fingerprint,
                    })
                }
                Ok(other) => {
                    return Err(ClientError::Usage(format!(
                        "unexpected response to DaemonInfo: {other:?}"
                    )))
                }
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.unwrap_or_else(|| {
            ClientError::DaemonUnreachable("start the yerd daemon first, then re-run".to_owned())
        }))
    }

    /// Candidate socket paths for the invoking user's daemon. Under sudo the
    /// process env points at root, so reconstruct from `SUDO_UID` (uid-based,
    /// home-independent); fall back to the normal resolution for logged-in root.
    fn socket_candidates() -> Vec<PathBuf> {
        use yerd_platform::{ActivePaths, Paths};
        if let Some(uid) = sudo_uid() {
            return user_socket_candidates(uid);
        }
        match ActivePaths::new().resolve() {
            Ok(dirs) => vec![dirs.runtime.join("yerd.sock")],
            Err(_) => Vec::new(),
        }
    }

    /// Pure: the uid-based socket paths the daemon would use (XDG runtime dir,
    /// then the `/tmp` fallback), mirroring `yerd_platform`'s Linux resolution.
    fn user_socket_candidates(uid: u32) -> Vec<PathBuf> {
        vec![
            PathBuf::from(format!("/run/user/{uid}/yerd/yerd.sock")),
            PathBuf::from(format!("/tmp/yerd-{uid}/yerd.sock")),
        ]
    }

    fn sudo_uid() -> Option<u32> {
        std::env::var("SUDO_UID").ok()?.parse().ok()
    }

    /// The uid that should own user-owned artefacts (the invoking user under
    /// sudo, else the current root).
    fn invoking_uid() -> u32 {
        sudo_uid().unwrap_or(0)
    }

    /// Locate `yerd-helper` and `yerdd` as siblings of the running `yerd`
    /// binary. Deriving `yerdd` here (not from IPC) means a forged daemon can't
    /// point root's setcap at an arbitrary binary.
    fn sibling_binaries() -> Result<(PathBuf, PathBuf), ClientError> {
        let exe = std::env::current_exe()
            .map_err(|e| ClientError::Usage(format!("cannot resolve current exe: {e}")))?;
        let dir = exe
            .parent()
            .ok_or_else(|| ClientError::Usage("current exe has no parent directory".to_owned()))?;
        Ok((dir.join("yerd-helper"), dir.join("yerdd")))
    }

    /// Require `path` to be owned by `uid` and not group/other-writable.
    fn require_user_owned(path: &Path, uid: u32) -> Result<(), ClientError> {
        use std::os::unix::fs::MetadataExt;
        let md = std::fs::metadata(path)
            .map_err(|e| ClientError::Usage(format!("cannot stat {}: {e}", path.display())))?;
        if md.uid() != uid {
            return Err(ClientError::Usage(format!(
                "{} is not owned by uid {uid}; refusing to trust it",
                path.display()
            )));
        }
        if md.mode() & 0o022 != 0 {
            return Err(ClientError::Usage(format!(
                "{} is group/world-writable; refusing to trust it",
                path.display()
            )));
        }
        Ok(())
    }

    fn spawn_helper(helper: &Path, inv: &HelperInvocation) -> Result<Option<i32>, ClientError> {
        let status = Command::new(helper)
            .env_clear()
            .args(inv.to_argv())
            .status()
            .map_err(|e| ClientError::Usage(format!("cannot run {}: {e}", helper.display())))?;
        Ok(status.code())
    }

    #[cfg(target_os = "linux")]
    fn is_root() -> bool {
        // /proc/self/status "Uid:\t<real>\t<effective>\t<saved>\t<fs>"
        std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|s| {
                s.lines().find_map(|l| {
                    let rest = l.strip_prefix("Uid:")?;
                    rest.split_whitespace().nth(1)?.parse::<u32>().ok()
                })
            })
            .is_some_and(|euid| euid == 0)
    }

    #[cfg(all(unix, not(target_os = "linux")))]
    fn is_root() -> bool {
        Command::new("id")
            .arg("-u")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| s.trim().parse::<u32>().ok())
            .is_some_and(|euid| euid == 0)
    }

    #[cfg(test)]
    #[allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
    mod tests {
        use super::*;

        fn facts() -> Facts {
            Facts {
                dns_addr: "127.0.0.1:1053".parse().unwrap(),
                tld: "test".into(),
                ca_path: PathBuf::from("/home/u/.local/share/yerd/ca.cert.pem"),
                ca_fingerprint: "ab".repeat(32),
            }
        }

        fn argv(inv: &HelperInvocation) -> Vec<String> {
            inv.to_argv()
                .into_iter()
                .map(|s| s.to_string_lossy().into_owned())
                .collect()
        }

        #[test]
        fn user_socket_candidates_are_uid_based() {
            let c = user_socket_candidates(1000);
            assert_eq!(c[0], PathBuf::from("/run/user/1000/yerd/yerd.sock"));
            assert_eq!(c[1], PathBuf::from("/tmp/yerd-1000/yerd.sock"));
        }

        #[test]
        fn trust_install_maps_to_install_ca() {
            let f = facts();
            let inv =
                plan_invocation(ElevateTarget::Trust, &f, Path::new("/x/yerdd"), false).unwrap();
            let a = argv(&inv);
            assert_eq!(a[0], "install-ca");
            assert!(a.contains(&"--pem".to_string()));
            assert!(a.contains(&f.ca_path.to_string_lossy().into_owned()));
            assert!(a.contains(&"--fingerprint".to_string()));
            assert!(a.contains(&"ab".repeat(32)));
        }

        #[test]
        fn trust_uninstall_maps_to_uninstall_ca() {
            let inv = plan_invocation(ElevateTarget::Trust, &facts(), Path::new("/x/yerdd"), true)
                .unwrap();
            assert_eq!(argv(&inv)[0], "uninstall-ca");
        }

        #[test]
        fn resolver_maps_to_install_resolver_with_addr() {
            let inv = plan_invocation(
                ElevateTarget::Resolver,
                &facts(),
                Path::new("/x/yerdd"),
                false,
            )
            .unwrap();
            let a = argv(&inv);
            assert_eq!(a[0], "install-resolver");
            assert!(a.contains(&"test".to_string()));
            assert!(a.contains(&"127.0.0.1:1053".to_string()));
        }

        #[test]
        fn ports_maps_to_setcap_on_local_yerdd() {
            let inv = plan_invocation(ElevateTarget::Ports, &facts(), Path::new("/x/yerdd"), false)
                .unwrap();
            let a = argv(&inv);
            assert_eq!(a[0], "setcap");
            assert!(a.contains(&"/x/yerdd".to_string()));
        }

        #[test]
        fn targets_none_expands_to_all_three_in_order() {
            assert_eq!(
                targets(None),
                vec![
                    ElevateTarget::Trust,
                    ElevateTarget::Resolver,
                    ElevateTarget::Ports
                ]
            );
            assert_eq!(
                targets(Some(ElevateTarget::Resolver)),
                vec![ElevateTarget::Resolver]
            );
        }
    }
}
