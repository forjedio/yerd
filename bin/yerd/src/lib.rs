//! Yerd CLI - a thin `yerd-ipc` client of the `yerdd` daemon.
//!
//! Binary-only crates don't expose a Rust API to integration tests under
//! `tests/`. This lib publishes the CLI's modules so `tests/cli_e2e.rs` can
//! drive the pure mapping (`map`) and the transport (`transport`) against a
//! daemon booted on a tempdir. All behaviour lives in the modules; `main.rs`
//! is a thin wrapper around [`run`].

#![forbid(unsafe_code)]

pub mod apply;
pub mod cli;
#[cfg(unix)]
pub mod composer_shim;
#[cfg(unix)]
pub mod cover_shim;
pub mod elevate;
pub mod error;
#[cfg(unix)]
pub mod laravel_shim;
pub mod map;
pub mod path_cmd;
#[cfg(unix)]
pub mod shim;
pub mod transport;
pub mod uninstall;

use std::process::ExitCode;

pub use error::ClientError;

use cli::{Cli, Command};

/// Map the parsed command to a request, exchange it with the daemon, and
/// render the response. Returns the process exit code:
/// `0` success, `1` daemon error response, `2` usage error, `69` daemon
/// unreachable, `74` other transport/IO failure.
pub async fn run(cli: Cli) -> ExitCode {
    match &cli.command {
        Command::Elevate { target } => return elevate::run_elevate(*target, false).await,
        Command::Unelevate { target } => return elevate::run_elevate(*target, true).await,
        Command::Path { action } => return path_cmd::run(*action),
        Command::Uninstall { target: None, yes } => return uninstall::run(*yes),
        Command::Install {
            target: crate::cli::InstallTarget::Tool { id },
        } if !cli.json => return stream_install_tool(id, cli.json).await,
        Command::Tunnel {
            action: crate::cli::TunnelAction::Install,
        } if !cli.json => {
            return stream_tunnel_job(yerd_ipc::Request::InstallCloudflaredStreamed).await
        }
        Command::Tunnel {
            action: crate::cli::TunnelAction::Login,
        } if !cli.json => return stream_tunnel_job(yerd_ipc::Request::CloudflaredLogin).await,
        Command::Update {
            target: None,
            yes: true,
            edge,
            stable,
            force,
        } => return run_self_update_apply(cli.json, *edge, *stable, *force).await,
        _ => {}
    }

    let req = match map::to_request(&cli.command)
        .map(canonicalize_unpark)
        .and_then(canonicalize_db_paths)
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("yerd: {e}");
            return ExitCode::from(2);
        }
    };

    match transport::exchange(&req).await {
        Ok(resp) => {
            let r = map::render(&resp, cli.json);
            if !r.stdout.is_empty() {
                println!("{}", r.stdout);
            }
            if !r.stderr.is_empty() {
                eprintln!("{}", r.stderr);
            }
            if !cli.json && r.code == 0 && matches!(cli.command, Command::Use { version: None, .. })
            {
                print_php_path_hint();
            }
            if !cli.json && r.code == 0 {
                if let Command::Update {
                    target: None,
                    yes: false,
                    edge,
                    stable,
                    ..
                } = &cli.command
                {
                    if *edge || *stable {
                        let ch = if *edge { "edge" } else { "stable" };
                        println!(
                            "\nyerd: showing the {ch} channel; your saved preference is \
                             unchanged - add --yes to switch"
                        );
                    }
                }
            }
            if r.code == 0
                && matches!(
                    cli.command,
                    Command::Install {
                        target: crate::cli::InstallTarget::Tool { .. }
                    }
                )
            {
                path_cmd::ensure_installed_after_tool(cli.json);
            }
            ExitCode::from(r.code)
        }
        Err(e @ ClientError::DaemonUnreachable(_)) => {
            if matches!(cli.command, Command::Doctor { .. }) {
                let resp = daemon_down_response();
                let r = map::render(&resp, cli.json);
                if !r.stdout.is_empty() {
                    println!("{}", r.stdout);
                }
                return ExitCode::from(r.code);
            }
            eprintln!("yerd: {e}");
            ExitCode::from(69)
        }
        Err(e) => {
            eprintln!("yerd: {e}");
            ExitCode::from(74)
        }
    }
}

/// `yerd update --yes`: the self-update apply path.
///
/// Persists the channel when `--edge`/`--stable` is given, checks the channel,
/// and when a newer version is available asks the daemon to download + verify
/// the artifact ([`Request::StageUpdate`]) and then applies it **in-process**
/// (the CLI is a short-lived terminal process: it swaps the bundle it runs from,
/// off its old inode, then exits). The detached-subprocess applier is only for
/// the GUI, which must quit during the swap.
#[allow(clippy::too_many_lines, clippy::fn_params_excessive_bools)]
async fn run_self_update_apply(json: bool, edge: bool, stable: bool, force: bool) -> ExitCode {
    use yerd_ipc::{Request, Response};

    if json {
        eprintln!("yerd: --json is not supported with `update --yes` (apply); use it for the check-only `yerd update`");
        return ExitCode::from(2);
    }

    let channel_override = map::channel_from_flags(edge, stable);

    if channel_override.is_some() {
        let name = if edge { "edge" } else { "stable" };
        match transport::exchange(&Request::SetUpdateChannel {
            channel: channel_override.unwrap_or(yerd_ipc::Channel::Stable),
        })
        .await
        {
            Ok(Response::Ok) => {
                if !json {
                    println!("yerd: update channel set to {name}");
                }
            }
            Ok(Response::Error { message, .. }) => {
                eprintln!("yerd: {message}");
                return ExitCode::from(1);
            }
            Ok(_) => {
                eprintln!("yerd: unexpected response setting update channel");
                return ExitCode::from(74);
            }
            Err(e @ ClientError::DaemonUnreachable(_)) => {
                eprintln!("yerd: {e}");
                return ExitCode::from(69);
            }
            Err(e) => {
                eprintln!("yerd: {e}");
                return ExitCode::from(74);
            }
        }
    }

    let status = match transport::exchange(&Request::CheckUpdate {
        channel: channel_override,
    })
    .await
    {
        Ok(Response::UpdateStatus {
            current,
            latest_stable,
            available,
            target,
            ahead_of_stable,
            ..
        }) => (current, latest_stable, available, target, ahead_of_stable),
        Ok(Response::Error { message, .. }) => {
            eprintln!("yerd: {message}");
            return ExitCode::from(1);
        }
        Ok(_) => {
            eprintln!("yerd: unexpected response checking for updates");
            return ExitCode::from(74);
        }
        Err(e @ ClientError::DaemonUnreachable(_)) => {
            eprintln!("yerd: {e}");
            return ExitCode::from(69);
        }
        Err(e) => {
            eprintln!("yerd: {e}");
            return ExitCode::from(74);
        }
    };
    let (current, latest_stable, available, target, ahead_of_stable) = status;

    if !available {
        if ahead_of_stable && force {
            println!(
                "yerd: you're on pre-release {current} (ahead of stable {}); automated \
                 downgrade isn't supported yet - reinstall the stable build manually",
                latest_stable.as_deref().unwrap_or("unknown")
            );
        } else if ahead_of_stable {
            println!(
                "yerd: on pre-release {current}, ahead of stable {} - staying put (use --force \
                 to force a downgrade once supported)",
                latest_stable.as_deref().unwrap_or("unknown")
            );
        } else {
            println!("yerd: already up to date ({current})");
        }
        return ExitCode::SUCCESS;
    }

    if !json {
        println!(
            "yerd: downloading and verifying {}…",
            target.as_deref().unwrap_or("the update")
        );
    }
    let (path, kind) = match transport::exchange(&Request::StageUpdate {
        channel: channel_override,
    })
    .await
    {
        Ok(Response::Staged { path, kind, .. }) => (path, kind),
        Ok(Response::Error { message, .. }) => {
            eprintln!("yerd: {message}");
            return ExitCode::from(1);
        }
        Ok(_) => {
            eprintln!("yerd: unexpected response staging the update");
            return ExitCode::from(74);
        }
        Err(e @ ClientError::DaemonUnreachable(_)) => {
            eprintln!("yerd: {e}");
            return ExitCode::from(69);
        }
        Err(e) => {
            eprintln!("yerd: {e}");
            return ExitCode::from(74);
        }
    };

    tokio::task::spawn_blocking(move || apply::run(std::path::Path::new(&path), kind, false))
        .await
        .unwrap_or_else(|e| {
            eprintln!("yerd: applier task failed: {e}");
            ExitCode::from(74)
        })
}

/// Install a dev tool as a streamed job, printing its output line by line until
/// the job reaches a terminal state. Mirrors the GUI's streamed install.
async fn stream_install_tool(id: &str, json: bool) -> ExitCode {
    use std::time::Duration;
    use yerd_ipc::{JobState, Request, Response};

    let job_id = match transport::exchange(&Request::InstallToolStreamed {
        tool: id.to_owned(),
    })
    .await
    {
        Ok(Response::JobStarted { job_id }) => job_id,
        Ok(Response::Error { message, .. }) => {
            eprintln!("yerd: {message}");
            return ExitCode::from(1);
        }
        Ok(_) => {
            eprintln!("yerd: unexpected response starting install");
            return ExitCode::from(74);
        }
        Err(e @ ClientError::DaemonUnreachable(_)) => {
            eprintln!("yerd: {e}");
            return ExitCode::from(69);
        }
        Err(e) => {
            eprintln!("yerd: {e}");
            return ExitCode::from(74);
        }
    };

    let mut cursor = 0u64;
    loop {
        match transport::exchange(&Request::JobStatus {
            job_id: job_id.clone(),
            cursor,
        })
        .await
        {
            Ok(Response::JobProgress {
                state,
                log,
                next_cursor,
                error,
                ..
            }) => {
                for line in &log {
                    println!("{line}");
                }
                cursor = next_cursor;
                match state {
                    JobState::Running => tokio::time::sleep(Duration::from_millis(400)).await,
                    JobState::Succeeded => {
                        path_cmd::ensure_installed_after_tool(json);
                        return ExitCode::SUCCESS;
                    }
                    JobState::Failed => {
                        if let Some(e) = error {
                            eprintln!("yerd: {e}");
                        }
                        return ExitCode::from(1);
                    }
                    JobState::Cancelled => {
                        eprintln!("yerd: install cancelled");
                        return ExitCode::from(1);
                    }
                }
            }
            Ok(Response::Error { message, .. }) => {
                eprintln!("yerd: {message}");
                return ExitCode::from(1);
            }
            Ok(_) => {
                eprintln!("yerd: unexpected response polling install");
                return ExitCode::from(74);
            }
            Err(e @ ClientError::DaemonUnreachable(_)) => {
                eprintln!("yerd: {e}");
                return ExitCode::from(69);
            }
            Err(e) => {
                eprintln!("yerd: {e}");
                return ExitCode::from(74);
            }
        }
    }
}

/// Run a streamed tunnel job (`cloudflared` install or account login), printing
/// progress lines (including the login auth URL) as they arrive. Mirrors
/// [`stream_install_tool`].
async fn stream_tunnel_job(req: yerd_ipc::Request) -> ExitCode {
    use std::time::Duration;
    use yerd_ipc::{JobState, Request, Response};

    let job_id = match transport::exchange(&req).await {
        Ok(Response::JobStarted { job_id }) => job_id,
        Ok(Response::Error { message, .. }) => {
            eprintln!("yerd: {message}");
            return ExitCode::from(1);
        }
        Ok(_) => {
            eprintln!("yerd: unexpected response starting install");
            return ExitCode::from(74);
        }
        Err(e @ ClientError::DaemonUnreachable(_)) => {
            eprintln!("yerd: {e}");
            return ExitCode::from(69);
        }
        Err(e) => {
            eprintln!("yerd: {e}");
            return ExitCode::from(74);
        }
    };

    let mut cursor = 0u64;
    loop {
        match transport::exchange(&Request::JobStatus {
            job_id: job_id.clone(),
            cursor,
        })
        .await
        {
            Ok(Response::JobProgress {
                state,
                log,
                next_cursor,
                error,
                ..
            }) => {
                for line in &log {
                    println!("{line}");
                }
                cursor = next_cursor;
                match state {
                    JobState::Running => tokio::time::sleep(Duration::from_millis(400)).await,
                    JobState::Succeeded => return ExitCode::SUCCESS,
                    JobState::Failed => {
                        if let Some(e) = error {
                            eprintln!("yerd: {e}");
                        }
                        return ExitCode::from(1);
                    }
                    JobState::Cancelled => {
                        eprintln!("yerd: install cancelled");
                        return ExitCode::from(1);
                    }
                }
            }
            Ok(Response::Error { message, .. }) => {
                eprintln!("yerd: {message}");
                return ExitCode::from(1);
            }
            Ok(_) => {
                eprintln!("yerd: unexpected response polling install");
                return ExitCode::from(74);
            }
            Err(e @ ClientError::DaemonUnreachable(_)) => {
                eprintln!("yerd: {e}");
                return ExitCode::from(69);
            }
            Err(e) => {
                eprintln!("yerd: {e}");
                return ExitCode::from(74);
            }
        }
    }
}

/// Best-effort: rewrite an `Unpark` request's path to its canonical form so a
/// relative or symlinked path the user typed matches the canonical string the
/// daemon stored when the directory was parked. The daemon matches `unpark`
/// *exactly* (it deliberately does not canonicalise - so a directory deleted
/// from disk is still removable by its exact stored path); doing it here, at the
/// I/O boundary, keeps `map::to_request` pure. A path that can't be canonicalised
/// (e.g. already deleted) is left exactly as typed.
fn canonicalize_unpark(req: yerd_ipc::Request) -> yerd_ipc::Request {
    if let yerd_ipc::Request::Unpark { path } = &req {
        if let Ok(canon) = std::fs::canonicalize(path) {
            return yerd_ipc::Request::Unpark {
                path: canon.to_string_lossy().into_owned(),
            };
        }
    }
    req
}

/// Absolutise the file path of a `BackupDatabase`/`RestoreDatabase` request against
/// the user's current directory before it reaches the daemon - the daemon's own cwd
/// differs from the user's shell, so a relative path would otherwise resolve in the
/// wrong place. Done here, at the I/O boundary, to keep `map::to_request` pure.
///
/// Restore requires the source file to exist (canonicalise, fail loudly if missing);
/// backup's destination does not exist yet, so it is merely made absolute.
fn canonicalize_db_paths(req: yerd_ipc::Request) -> Result<yerd_ipc::Request, ClientError> {
    use yerd_ipc::Request;
    match req {
        Request::RestoreDatabase {
            service,
            name,
            path,
        } => {
            let path = std::fs::canonicalize(&path).map_err(|e| {
                ClientError::Usage(format!("cannot read backup file {}: {e}", path.display()))
            })?;
            Ok(Request::RestoreDatabase {
                service,
                name,
                path,
            })
        }
        Request::BackupDatabase {
            service,
            name,
            path,
        } => Ok(Request::BackupDatabase {
            service,
            name,
            path: absolutise(&path)?,
        }),
        other => Ok(other),
    }
}

/// Make a (possibly relative) path absolute by joining it onto the current directory.
/// Does not require the path to exist (used for a backup destination).
fn absolutise(path: &std::path::Path) -> Result<std::path::PathBuf, ClientError> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    let cwd = std::env::current_dir()
        .map_err(|e| ClientError::Usage(format!("cannot resolve current directory: {e}")))?;
    Ok(cwd.join(path))
}

/// A synthetic `daemon_down` FAIL diagnosis, used when `yerd doctor` can't reach
/// the daemon. Routed through `map::render` so it honours `--json` and exits 1.
fn daemon_down_response() -> yerd_ipc::Response {
    yerd_ipc::Response::Diagnoses {
        items: vec![yerd_ipc::Diagnosis {
            code: yerd_ipc::DiagnosisCode::DaemonDown,
            severity: yerd_ipc::Severity::Fail,
            title: "Daemon not running".to_owned(),
            detail: "Could not reach the yerd daemon over its IPC socket.".to_owned(),
            remedy: Some("start the daemon: yerdd".to_owned()),
        }],
    }
}

/// Print where the managed `php` shim lives and warn if another `php` already
/// shadows it on `PATH`. Best-effort: silently does nothing if dirs can't be
/// resolved.
fn print_php_path_hint() {
    use yerd_platform::{ActivePaths, Paths};
    let Ok(dirs) = ActivePaths::new().resolve() else {
        return;
    };
    let bin = dirs.data.join("bin");
    println!(
        "→ ensure {} is on your PATH for the `php` command",
        bin.display()
    );
    println!("  (also provides `php<ver>` and `phpcover`/`php<ver>cover` for pcov coverage)");
    if let Some(existing) = first_php_on_path() {
        if existing != bin.join("php") {
            println!(
                "  note: `php` currently resolves to {} - put {} earlier on PATH to override",
                existing.display(),
                bin.display()
            );
        }
    }
}

/// First `php` executable found on `PATH`, if any.
fn first_php_on_path() -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join("php"))
        .find(|candidate| candidate.is_file())
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
    use std::path::{Path, PathBuf};
    use yerd_ipc::Request;

    // ─── canonicalize_unpark ────────────────────────────────────────

    #[test]
    fn canonicalize_unpark_resolves_existing_path() {
        let tmp = tempfile::tempdir().unwrap();
        let nested = tmp.path().join("sub");
        std::fs::create_dir(&nested).unwrap();
        let req = Request::Unpark {
            path: nested.to_string_lossy().into_owned(),
        };
        let out = canonicalize_unpark(req);
        let Request::Unpark { path } = out else {
            panic!("expected Unpark");
        };
        let canon = std::fs::canonicalize(&nested).unwrap();
        assert_eq!(path, canon.to_string_lossy());
    }

    #[test]
    fn canonicalize_unpark_leaves_missing_path_untouched() {
        let raw = "/no/such/dir/that/exists/anywhere-xyz";
        let req = Request::Unpark {
            path: raw.to_owned(),
        };
        match canonicalize_unpark(req) {
            Request::Unpark { path } => assert_eq!(path, raw),
            _ => panic!("expected Unpark"),
        }
    }

    #[test]
    fn canonicalize_unpark_passes_through_other_requests() {
        assert_eq!(canonicalize_unpark(Request::Ping), Request::Ping);
        let listed = canonicalize_unpark(Request::ListSites);
        assert_eq!(listed, Request::ListSites);
    }

    // ─── canonicalize_db_paths ──────────────────────────────────────

    #[test]
    fn canonicalize_db_paths_restore_existing_file_is_canonicalised() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("dump.sql");
        std::fs::write(&file, b"-- sql").unwrap();
        let req = Request::RestoreDatabase {
            service: "mysql".into(),
            name: "app".into(),
            path: file.clone(),
        };
        let out = canonicalize_db_paths(req).unwrap();
        let Request::RestoreDatabase {
            path,
            service,
            name,
        } = out
        else {
            panic!("expected RestoreDatabase");
        };
        assert_eq!(service, "mysql");
        assert_eq!(name, "app");
        assert_eq!(path, std::fs::canonicalize(&file).unwrap());
    }

    #[test]
    fn canonicalize_db_paths_restore_missing_file_is_usage_error() {
        let req = Request::RestoreDatabase {
            service: "mysql".into(),
            name: "app".into(),
            path: PathBuf::from("/no/such/backup-file-xyz.sql"),
        };
        let err = canonicalize_db_paths(req).unwrap_err();
        assert!(matches!(err, ClientError::Usage(_)), "got: {err:?}");
        assert!(err.to_string().contains("cannot read backup file"));
    }

    #[test]
    fn canonicalize_db_paths_backup_relative_is_absolutised() {
        let req = Request::BackupDatabase {
            service: "mysql".into(),
            name: "app".into(),
            path: PathBuf::from("rel/app.sql"),
        };
        let out = canonicalize_db_paths(req).unwrap();
        let Request::BackupDatabase { path, .. } = out else {
            panic!("expected BackupDatabase");
        };
        assert!(
            path.is_absolute(),
            "backup path should be absolutised: {path:?}"
        );
        assert!(path.ends_with("rel/app.sql"));
    }

    #[test]
    fn canonicalize_db_paths_backup_absolute_is_unchanged() {
        let abs = PathBuf::from("/var/tmp/app.sql");
        let req = Request::BackupDatabase {
            service: "mysql".into(),
            name: "app".into(),
            path: abs.clone(),
        };
        let out = canonicalize_db_paths(req).unwrap();
        match out {
            Request::BackupDatabase { path, .. } => assert_eq!(path, abs),
            _ => panic!("expected BackupDatabase"),
        }
    }

    #[test]
    fn canonicalize_db_paths_other_request_passes_through() {
        let out = canonicalize_db_paths(Request::Ping).unwrap();
        assert_eq!(out, Request::Ping);
    }

    // ─── absolutise ─────────────────────────────────────────────────

    #[test]
    fn absolutise_returns_absolute_path_unchanged() {
        let abs = Path::new("/etc/hosts");
        assert_eq!(absolutise(abs).unwrap(), abs.to_path_buf());
    }

    #[test]
    fn absolutise_joins_relative_onto_cwd() {
        let rel = Path::new("some/where.sql");
        let out = absolutise(rel).unwrap();
        assert!(out.is_absolute());
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(out, cwd.join(rel));
    }

    // ─── daemon_down_response ───────────────────────────────────────

    #[test]
    fn daemon_down_response_is_single_fail_diagnosis() {
        match daemon_down_response() {
            yerd_ipc::Response::Diagnoses { items } => {
                assert_eq!(items.len(), 1);
                assert_eq!(items[0].code, yerd_ipc::DiagnosisCode::DaemonDown);
                assert_eq!(items[0].severity, yerd_ipc::Severity::Fail);
                assert!(items[0].remedy.is_some());
            }
            other => panic!("expected Diagnoses, got {other:?}"),
        }
    }

    // ─── PATH helpers ───────────────────────────────────────────────

    /// The result depends on the host PATH, so any returned candidate (if one
    /// exists) must end in `php` and be a file.
    #[test]
    fn first_php_on_path_returns_option() {
        if let Some(p) = first_php_on_path() {
            assert!(p.ends_with("php"));
            assert!(p.is_file());
        }
    }

    /// Best-effort printer; must not panic regardless of environment.
    #[test]
    fn print_php_path_hint_runs() {
        print_php_path_hint();
    }
}
