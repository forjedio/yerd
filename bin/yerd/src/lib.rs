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
pub mod cli_shim;
#[cfg(unix)]
pub mod composer_shim;
#[cfg(unix)]
pub mod cover_shim;
pub mod elevate;
pub mod error;
#[cfg(unix)]
pub mod laravel_shim;
pub mod map;
pub mod mcp_cmd;
pub mod path_cmd;
#[cfg(unix)]
pub mod shim;
pub mod transport;
pub mod uninstall;
#[cfg(unix)]
pub mod wp_shim;

use std::process::ExitCode;

pub use error::ClientError;

use cli::{Cli, Command};

/// Map the parsed command to a request, exchange it with the daemon, and
/// render the response. Returns the process exit code:
/// `0` success, `1` daemon error response, `2` usage error, `69` daemon
/// unreachable, `74` other transport/IO failure.
#[allow(clippy::too_many_lines)]
pub async fn run(cli: Cli) -> ExitCode {
    match &cli.command {
        Command::Elevate { target } => return elevate::run_elevate(*target, false).await,
        Command::Unelevate { target } => return elevate::run_elevate(*target, true).await,
        Command::Path { action } => return path_cmd::run(*action),
        Command::Mcp => return mcp_cmd::run().await,
        #[cfg_attr(not(unix), allow(unused_variables))]
        Command::Coverage { args } => {
            #[cfg(unix)]
            {
                return cover_shim::run_coverage(args);
            }
            #[cfg(not(unix))]
            {
                eprintln!("yerd: coverage is only available on macOS and Linux");
                return ExitCode::from(2);
            }
        }
        Command::Domain {
            action: crate::cli::DomainAction::List { site },
        } => return run_domain_list(site.as_deref(), cli.json).await,
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
        Command::Lan {
            action: crate::cli::LanAction::Enable,
        } => return run_lan_toggle(true, cli.json).await,
        Command::Lan {
            action: crate::cli::LanAction::Disable,
        } => return run_lan_toggle(false, cli.json).await,
        Command::Lan {
            action: crate::cli::LanAction::Status,
        } => return run_lan_status(cli.json).await,
        _ => {}
    }

    let req = match &cli.command {
        Command::Link { name_or_path, path } => {
            resolve_link(name_or_path.as_deref(), path.as_deref())
        }
        _ => map::to_request(&cli.command)
            .map(canonicalize_unpark)
            .and_then(canonicalize_db_paths),
    };
    let req = match req {
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
        Err(e) if e.is_daemon_down() => {
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

/// `yerd lan enable|disable`: a two-request flow that persists the flag then
/// **enforces** the daemon restart that re-binds the listeners (a listen
/// socket's bind address is fixed at bind time, so a hint is not enough for a
/// security-toggling command). Captures `boot_id` before, sends `RestartDaemon`,
/// and polls `Status` across the re-exec socket gap until `boot_id` changes.
async fn run_lan_toggle(enabled: bool, json: bool) -> ExitCode {
    use yerd_ipc::{Request, Response};

    let before = match fetch_boot_id().await {
        Ok(id) => id,
        Err(e) => {
            eprintln!("yerd: {e}");
            return ExitCode::from(69);
        }
    };

    match transport::exchange(&Request::SetLanEnabled { enabled }).await {
        Ok(Response::Ok) => {}
        Ok(Response::Error { message, .. }) => {
            eprintln!("yerd: {message}");
            return ExitCode::from(1);
        }
        Ok(other) => {
            eprintln!("yerd: unexpected response: {other:?}");
            return ExitCode::from(1);
        }
        Err(e) => {
            eprintln!("yerd: {e}");
            return ExitCode::from(69);
        }
    }

    if let Err(e) = restart_and_await_boot_change(before).await {
        eprintln!(
            "yerd: LAN {} was saved, but the daemon restart could not be confirmed: {e}",
            if enabled { "enable" } else { "disable" }
        );
        eprintln!("      restart it manually so the change takes effect.");
        return ExitCode::from(74);
    }

    if json {
        println!("{{\"lan_enabled\":{enabled},\"restarted\":true}}");
        return ExitCode::SUCCESS;
    }

    if enabled {
        println!("LAN exposure enabled and the daemon restarted.");
        println!();
        if cfg!(target_os = "macos") {
            println!("Next: install the LAN redirect (one-time, needs root):");
            println!("    sudo yerd elevate lan");
            println!(
                "(this also requires `sudo yerd elevate ports` — run it first if you haven't)."
            );
        } else {
            println!("Ensure `sudo yerd elevate ports` has been run so 80/443 bind, and open");
            println!("80/443/1053 to your LAN in the host firewall (see `yerd lan status`).");
        }
        println!();
        println!("Then provision a device with:  yerd remote-setup");
        println!("Check exposure at any time with:  yerd lan status");
    } else {
        println!(
            "LAN exposure disabled and the daemon restarted (listeners are back on loopback)."
        );
        if cfg!(target_os = "macos") {
            println!();
            println!("The macOS pf LAN redirect is separate privileged state — remove it with:");
            println!("    sudo yerd unelevate lan");
            println!("Until you do, `yerd lan status` will flag it as residual.");
        }
    }
    ExitCode::SUCCESS
}

/// `yerd lan status`: a LAN-focused view of the daemon's `Status`, showing
/// configured-vs-effective state so "enabled but not exposed" (and, on macOS,
/// "disabled but pf still redirecting") are both visible.
async fn run_lan_status(json: bool) -> ExitCode {
    use yerd_ipc::{Request, Response};
    let report = match transport::exchange(&Request::Status).await {
        Ok(Response::Status { report }) => report,
        Ok(other) => {
            eprintln!("yerd: unexpected response: {other:?}");
            return ExitCode::from(1);
        }
        Err(e) => {
            eprintln!("yerd: {e}");
            return ExitCode::from(69);
        }
    };

    if json {
        let ip = report
            .lan_ip
            .map_or_else(|| "null".to_owned(), |i| format!("\"{i}\""));
        let bound = report
            .lan_setup_bound
            .map_or_else(|| "null".to_owned(), |b| b.to_string());
        println!(
            "{{\"lan_enabled\":{},\"lan_ip\":{ip},\"lan_setup_bound\":{bound}}}",
            report.lan_enabled
        );
        return ExitCode::SUCCESS;
    }

    if !report.lan_enabled {
        println!("LAN exposure: OFF (sites are served on loopback only).");
        println!("Enable it with:  yerd lan enable");
        #[cfg(target_os = "macos")]
        println!(
            "Note: if you previously ran `sudo yerd elevate lan`, remove the residual pf rule \
             with `sudo yerd unelevate lan`."
        );
        return ExitCode::SUCCESS;
    }

    println!("LAN exposure: ON (configured).");
    match report.lan_ip {
        Some(ip) => println!("  LAN address:      {ip}"),
        None => println!("  LAN address:      <discovery failed — answers fall back to loopback>"),
    }
    match report.lan_setup_bound {
        Some(true) => println!(
            "  Bootstrap:        listening (run `yerd remote-setup` to provision a device)"
        ),
        Some(false) => {
            println!("  Bootstrap:        NOT bound (port busy? check `lan_setup_port`)");
        }
        None => {}
    }
    if cfg!(target_os = "macos") {
        println!(
            "  macOS redirect:   run `sudo yerd elevate lan` (and `elevate ports`) if 80/443 \
             aren't reachable from the LAN yet."
        );
    } else {
        println!(
            "  Linux:            ensure `sudo yerd elevate ports` is applied and the host \
             firewall allows 80/443/1053 from your LAN."
        );
    }
    ExitCode::SUCCESS
}

/// Read the running daemon's `boot_id` (a per-process random id used to detect a
/// completed restart across the pid-preserving re-exec).
async fn fetch_boot_id() -> Result<Option<u64>, ClientError> {
    use yerd_ipc::{Request, Response};
    match transport::exchange(&Request::Status).await? {
        Response::Status { report } => Ok(report.boot_id),
        other => Err(ClientError::Usage(format!(
            "unexpected response to Status: {other:?}"
        ))),
    }
}

/// Send `RestartDaemon`, then poll `Status` (tolerating the transient
/// connection failure while the daemon re-execs) until `boot_id` differs from
/// `before` or a bounded timeout elapses.
async fn restart_and_await_boot_change(before: Option<u64>) -> Result<(), ClientError> {
    use yerd_ipc::Request;
    // The daemon writes `Ok` and flushes *before* it re-execs, so this returns
    // normally; a transient error is tolerated too (the socket may already be
    // tearing down).
    let _ = transport::exchange(&Request::RestartDaemon).await;

    let deadline = std::time::Duration::from_secs(15);
    let step = std::time::Duration::from_millis(200);
    let mut waited = std::time::Duration::ZERO;
    loop {
        tokio::time::sleep(step).await;
        waited += step;
        if let Ok(now) = fetch_boot_id().await {
            if now != before && now.is_some() {
                return Ok(());
            }
        }
        if waited >= deadline {
            return Err(ClientError::Usage(
                "timed out waiting for the daemon to come back up".to_owned(),
            ));
        }
    }
}

/// `yerd domain list [site]`: a local two-request flow. Needs the TLD (via
/// `DaemonInfo`) to render an effectively-default site's `{name}.{tld}` domain,
/// then lists sites and renders a domain-focused view.
async fn run_domain_list(site: Option<&str>, json: bool) -> ExitCode {
    use yerd_ipc::{Request, Response};
    let tld = match transport::exchange(&Request::DaemonInfo).await {
        Ok(Response::Info { tld, .. }) => tld,
        Ok(_) => {
            eprintln!("yerd: unexpected daemon response");
            return ExitCode::from(74);
        }
        Err(e) if e.is_daemon_down() => {
            eprintln!("yerd: {e}");
            return ExitCode::from(69);
        }
        Err(e) => {
            eprintln!("yerd: {e}");
            return ExitCode::from(74);
        }
    };

    match transport::exchange(&Request::ListSites).await {
        Ok(Response::Sites { sites }) => {
            let r = map::render_domains(&sites, &tld, site, json);
            if !r.stdout.is_empty() {
                println!("{}", r.stdout);
            }
            if !r.stderr.is_empty() {
                eprintln!("{}", r.stderr);
            }
            ExitCode::from(r.code)
        }
        Ok(other) => {
            let r = map::render(&other, json);
            if !r.stdout.is_empty() {
                println!("{}", r.stdout);
            }
            if !r.stderr.is_empty() {
                eprintln!("{}", r.stderr);
            }
            ExitCode::from(r.code)
        }
        Err(e) if e.is_daemon_down() => {
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
            Err(e) if e.is_daemon_down() => {
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
        Err(e) if e.is_daemon_down() => {
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
        Err(e) if e.is_daemon_down() => {
            eprintln!("yerd: {e}");
            return ExitCode::from(69);
        }
        Err(e) => {
            eprintln!("yerd: {e}");
            return ExitCode::from(74);
        }
    };

    tokio::task::spawn_blocking(move || apply::run(std::path::Path::new(&path), kind, false, false))
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
        Err(e) if e.is_daemon_down() => {
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
            Err(e) if e.is_daemon_down() => {
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

    let noun = if matches!(req, Request::CloudflaredLogin) {
        "login"
    } else {
        "install"
    };

    let job_id = match transport::exchange(&req).await {
        Ok(Response::JobStarted { job_id }) => job_id,
        Ok(Response::Error { message, .. }) => {
            eprintln!("yerd: {message}");
            return ExitCode::from(1);
        }
        Ok(_) => {
            eprintln!("yerd: unexpected response starting {noun}");
            return ExitCode::from(74);
        }
        Err(e) if e.is_daemon_down() => {
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
                        eprintln!("yerd: {noun} cancelled");
                        return ExitCode::from(1);
                    }
                }
            }
            Ok(Response::Error { message, .. }) => {
                eprintln!("yerd: {message}");
                return ExitCode::from(1);
            }
            Ok(_) => {
                eprintln!("yerd: unexpected response polling {noun}");
                return ExitCode::from(74);
            }
            Err(e) if e.is_daemon_down() => {
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

/// Whether a single positional argument to `yerd link` should be treated as
/// a directory rather than a bare site name: contains a path separator, or
/// is `.`/`..`. Bare words (`yerd link project`) are always names, even if a
/// same-named subdirectory happens to exist.
fn looks_like_path(s: &str) -> bool {
    s == "." || s == ".." || s.contains('/') || s.contains(std::path::MAIN_SEPARATOR)
}

/// Resolve `yerd link`'s optional name/path into a concrete `Request::Link`.
/// Cwd-dependent (reads `std::env::current_dir()` when path is omitted) and
/// therefore not part of the pure `map::to_request` pipeline - kept here at
/// the I/O boundary, mirroring `absolutise`/`canonicalize_db_paths`.
///
/// Public so `tests/cli_e2e.rs` can drive the same CLI-side resolution this
/// crate's `run()` uses, then exchange the result with a real daemon.
pub fn resolve_link(
    name_or_path: Option<&str>,
    path: Option<&std::path::Path>,
) -> Result<yerd_ipc::Request, ClientError> {
    let explicit_path = path.or_else(|| {
        name_or_path
            .filter(|s| looks_like_path(s))
            .map(std::path::Path::new)
    });
    let explicit_name = if path.is_some() {
        name_or_path
    } else {
        name_or_path.filter(|s| !looks_like_path(s))
    };

    let resolved_path = match explicit_path {
        Some(p) => absolutise(p)?,
        None => std::env::current_dir()
            .map_err(|e| ClientError::Usage(format!("cannot resolve current directory: {e}")))?,
    };

    let resolved_name = if let Some(n) = explicit_name {
        map::validate_name(n)?;
        n.to_owned()
    } else {
        let normalized = normalize_lexically(&resolved_path);
        let folder = normalized
            .file_name()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();
        yerd_core::slugify_site_name(&folder).ok_or_else(|| {
            ClientError::Usage(format!(
                "cannot derive a site name from '{}'; run `yerd link <name> {}` to set one explicitly",
                resolved_path.display(),
                resolved_path.display(),
            ))
        })?
    };

    Ok(yerd_ipc::Request::Link {
        name: resolved_name,
        path: resolved_path,
    })
}

/// Lexically resolve `.`/`..` components out of `path` without touching the
/// filesystem (no `canonicalize` - `resolve_link` deliberately doesn't
/// require the target to exist), so `Path::file_name()` sees the folder
/// actually being linked. `Path::file_name()` already normalises a trailing
/// `.` away on its own, but a trailing `..` survives as-is (it can't tell
/// what it cancels without looking further back), so e.g. `yerd link ..`
/// would otherwise fail to derive the parent folder's name even though it's
/// well-defined.
fn normalize_lexically(path: &std::path::Path) -> std::path::PathBuf {
    use std::path::{Component, PathBuf};
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                } else {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
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

    // ─── looks_like_path ────────────────────────────────────────────

    #[test]
    fn looks_like_path_classifies_bare_words_vs_paths() {
        let cases: &[(&str, bool)] = &[
            ("project", false),
            ("my-app", false),
            (".", true),
            ("..", true),
            ("./project", true),
            ("~/sites/example", true),
            ("/abs/path", true),
        ];
        for (input, expected) in cases {
            assert_eq!(looks_like_path(input), *expected, "input {input:?}");
        }
    }

    // ─── resolve_link ───────────────────────────────────────────────

    #[test]
    fn resolve_link_no_args_uses_cwd_and_derives_name() {
        let req = resolve_link(None, None).unwrap();
        let Request::Link { name, path } = req else {
            panic!("expected Link");
        };
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(path, cwd);
        let folder = cwd.file_name().and_then(|s| s.to_str()).unwrap_or("");
        assert_eq!(Some(name), yerd_core::slugify_site_name(folder));
    }

    #[test]
    fn resolve_link_bare_word_uses_cwd_as_path() {
        let req = resolve_link(Some("myapp"), None).unwrap();
        let Request::Link { name, path } = req else {
            panic!("expected Link");
        };
        assert_eq!(name, "myapp");
        assert_eq!(path, std::env::current_dir().unwrap());
    }

    #[test]
    fn resolve_link_single_path_arg_derives_name() {
        let req = resolve_link(Some("../my-app"), None).unwrap();
        let Request::Link { name, path } = req else {
            panic!("expected Link");
        };
        assert_eq!(name, "my-app");
        assert!(path.is_absolute());
        assert!(path.ends_with("../my-app"));
    }

    #[test]
    fn resolve_link_explicit_name_and_path_unchanged() {
        let req = resolve_link(Some("blog"), Some(Path::new("rel/blog"))).unwrap();
        let Request::Link { name, path } = req else {
            panic!("expected Link");
        };
        assert_eq!(name, "blog");
        assert!(path.is_absolute());
        assert!(path.ends_with("rel/blog"));
    }

    /// "weird/???" looks like a path (contains '/'), so the name is derived
    /// from its final component "???", which slugifies to `None`.
    #[test]
    fn resolve_link_undecipherable_name_errors() {
        let err = resolve_link(Some("weird/???"), None).unwrap_err();
        assert!(matches!(err, ClientError::Usage(_)), "got: {err:?}");
    }

    #[test]
    fn resolve_link_rejects_invalid_explicit_name() {
        let err = resolve_link(Some("bad name"), Some(Path::new("/x"))).unwrap_err();
        assert!(matches!(err, ClientError::Usage(_)), "got: {err:?}");
    }

    /// "" doesn't look like a path, so it's routed to explicit-name
    /// validation, which rejects an empty name.
    #[test]
    fn resolve_link_empty_string_name_errors() {
        let err = resolve_link(Some(""), None).unwrap_err();
        assert!(matches!(err, ClientError::Usage(_)), "got: {err:?}");
    }

    /// "/" normalises to the filesystem root, which has no `Normal`
    /// component left to derive a name from.
    #[test]
    fn resolve_link_root_path_has_no_file_name_errors() {
        let err = resolve_link(Some("/"), None).unwrap_err();
        assert!(matches!(err, ClientError::Usage(_)), "got: {err:?}");
    }

    #[test]
    fn resolve_link_trailing_curdir_derives_name() {
        let req = resolve_link(Some("some/dir/."), None).unwrap();
        let Request::Link { name, .. } = req else {
            panic!("expected Link");
        };
        assert_eq!(name, "dir");
    }

    #[test]
    fn resolve_link_trailing_parentdir_derives_name() {
        let req = resolve_link(Some("some/parent/child/.."), None).unwrap();
        let Request::Link { name, .. } = req else {
            panic!("expected Link");
        };
        assert_eq!(name, "parent");
    }

    // ─── normalize_lexically ────────────────────────────────────────

    #[test]
    fn normalize_lexically_cases() {
        let cases: &[(&str, &str)] = &[
            ("/home/user/myapp/.", "/home/user/myapp"),
            ("/home/user/myapp/..", "/home/user"),
            ("/a/b/../c", "/a/c"),
            ("/../../foo", "/../../foo"),
            ("/", "/"),
        ];
        for (input, expected) in cases {
            assert_eq!(
                normalize_lexically(Path::new(input)),
                Path::new(expected),
                "input {input:?}"
            );
        }
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
