//! `CreateSite` — scaffold a new project (`laravel new`) then register it.
//!
//! Scaffolding runs far longer than one request/response round-trip and streams
//! output, so this runs as a background [job](crate::jobs): [`start`] spawns the
//! work and returns a [`Response::JobStarted`] immediately; the client polls
//! `JobStatus` for the streamed log + phase.
//!
//! The work, in order: **preflight** (validate, reserve the name, check the
//! toolchain, probe the target), build a **per-job PATH** that pins the chosen
//! PHP for the installer *and* the Composer it shells out to, **scaffold**
//! (direct `tokio::process` with piped stdio + a process group we can kill), and
//! **register** through the existing mutation path so the new site is served.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::watch;

use yerd_ipc::{
    AuthProvider, CreateSiteSpec, Database, ErrorCode, Framework, JobState, JsRuntime,
    LaravelOptions, Request, Response, StarterKit, Testing,
};

use crate::state::DaemonState;
use crate::tools::{self, Tool};

/// Hard cap on a single scaffold (Composer + optional `npm install && build`).
/// Hitting it kills the process group and fails the job.
const SCAFFOLD_TIMEOUT: Duration = Duration::from_secs(20 * 60);

/// Validate the request synchronously, then spawn the background job.
pub async fn start(spec: CreateSiteSpec, state: Arc<DaemonState>) -> Response {
    // Only Laravel today; the enum is `#[non_exhaustive]`, so a catch-all is
    // required and guards future variants.
    match &spec.framework {
        Framework::Laravel { .. } => {}
        _ => return error(ErrorCode::Internal, "unsupported framework".to_owned()),
    }

    // Validate (and lowercase) the name up front so a bad request fails now,
    // before any job/spinner. `Site::linked` applies the DNS-label rules; the
    // path is not validated at this layer, so any value works for the check.
    let name = match yerd_core::Site::linked(&spec.name, spec.parent_dir.clone(), spec.php) {
        Ok(site) => site.name().to_owned(),
        Err(e) => return error(ErrorCode::InvalidPath, e.to_string()),
    };

    let (job_id, cancel_rx) = state.jobs.create().await;
    let id = job_id.clone();
    tokio::spawn(async move {
        run_job(id, name, spec, state, cancel_rx).await;
    });
    Response::JobStarted { job_id }
}

/// Reserve the name, run the job, then always release the reservation + clean
/// the per-job scratch dir.
async fn run_job(
    id: String,
    name: String,
    spec: CreateSiteSpec,
    state: Arc<DaemonState>,
    cancel_rx: watch::Receiver<bool>,
) {
    // Reserve the name so two concurrent creates can't both scaffold it.
    if !state.reserved_names.lock().await.insert(name.clone()) {
        state
            .jobs
            .finish(
                &id,
                JobState::Failed,
                Some(format!("a site named \"{name}\" is already being created")),
            )
            .await;
        return;
    }

    let job_dir = state.dirs.cache.join(format!("create-{id}"));
    let outcome = run_inner(&id, &name, &spec, &job_dir, &state, cancel_rx).await;

    state.reserved_names.lock().await.remove(&name);
    let _ = std::fs::remove_dir_all(&job_dir);

    match outcome {
        Outcome::Succeeded => {
            state.jobs.set_phase(&id, "Done").await;
            state.jobs.finish(&id, JobState::Succeeded, None).await;
        }
        Outcome::Cancelled => {
            state.jobs.push_log(&id, "cancelled".to_owned()).await;
            state.jobs.finish(&id, JobState::Cancelled, None).await;
        }
        Outcome::Failed(msg) => {
            state.jobs.push_log(&id, format!("error: {msg}")).await;
            state.jobs.finish(&id, JobState::Failed, Some(msg)).await;
        }
    }
}

/// Terminal result of the scaffolding work.
enum Outcome {
    Succeeded,
    Failed(String),
    Cancelled,
}

async fn run_inner(
    id: &str,
    name: &str,
    spec: &CreateSiteSpec,
    job_dir: &Path,
    state: &Arc<DaemonState>,
    mut cancel_rx: watch::Receiver<bool>,
) -> Outcome {
    let Framework::Laravel { options } = &spec.framework else {
        return Outcome::Failed("unsupported framework".to_owned());
    };
    let dirs = &state.dirs;
    let project_dir = spec.parent_dir.join(name);

    // --- Preflight -----------------------------------------------------------
    state.jobs.set_phase(id, "Preflight").await;

    let php_cli = crate::php_install::cli_binary_path(dirs, spec.php);
    if !php_cli.is_file() {
        return Outcome::Failed(format!(
            "PHP {}.{} is not installed",
            spec.php.major, spec.php.minor
        ));
    }
    let composer_phar = tools::composer::phar_path(dirs);
    if !composer_phar.is_file() {
        return Outcome::Failed("Composer is not installed — install it first".to_owned());
    }
    let installer_bin = tools::laravel::installer_bin(dirs);
    if !installer_bin.is_file() {
        return Outcome::Failed(
            "the Laravel installer is not installed — install it first".to_owned(),
        );
    }

    // Target dir must be absent or empty (never `--force` over a user's files).
    if let Err(msg) = check_target_dir(&project_dir) {
        return Outcome::Failed(msg);
    }
    // Parent must exist and be writable by the daemon.
    if let Err(msg) = probe_writable(&spec.parent_dir) {
        return Outcome::Failed(msg);
    }

    // Install the chosen JS runtime if a kit needs it and it's missing.
    if let Err(msg) = ensure_js_runtime(id, options.js, state).await {
        return Outcome::Failed(msg);
    }

    // Build the per-job PATH bin dir that pins the chosen PHP.
    let job_bin = match build_job_bin(job_dir, &php_cli, &composer_phar) {
        Ok(b) => b,
        Err(msg) => return Outcome::Failed(msg),
    };
    let path_env = composed_path(&job_bin, &tools::bin_dir(dirs));
    let composer_home = tools::laravel::composer_home(dirs);

    // `git` is effectively required by the installer (kits + `--git` run `git
    // init`); fail clearly in preflight rather than mid-scaffold.
    if needs_git(options) && !git_available(&path_env).await {
        return Outcome::Failed(
            "git was not found on PATH — install git to use a starter kit or git init".to_owned(),
        );
    }

    // --- Scaffold ------------------------------------------------------------
    state.jobs.set_phase(id, "Scaffolding").await;
    let args = build_new_args(name, options);
    state
        .jobs
        .push_log(id, format!("$ laravel {}", args.join(" ")))
        .await;

    let scaffold = run_scaffold(
        id,
        &php_cli,
        &installer_bin,
        &args,
        &spec.parent_dir,
        &path_env,
        &composer_home,
        state,
        &mut cancel_rx,
    )
    .await;
    match scaffold {
        ScaffoldOutcome::Ok => {}
        ScaffoldOutcome::Failed(msg) => {
            let _ = std::fs::remove_dir_all(&project_dir);
            return Outcome::Failed(msg);
        }
        ScaffoldOutcome::Cancelled => {
            let _ = std::fs::remove_dir_all(&project_dir);
            return Outcome::Cancelled;
        }
    }

    // --- Register ------------------------------------------------------------
    state.jobs.set_phase(id, "Registering").await;
    if let Err(msg) = register(name, &spec.parent_dir, &project_dir, spec, state).await {
        // The project files are valid; leave them on disk but report the failure
        // (the user can retry registration via Link). Do NOT delete their code.
        return Outcome::Failed(format!("scaffolded, but registration failed: {msg}"));
    }
    state
        .jobs
        .push_log(id, format!("serving https://{name}.test"))
        .await;
    Outcome::Succeeded
}

/// Build the `laravel new …` argument vector (after the installer binary).
/// Pure — unit-tested.
fn build_new_args(name: &str, o: &LaravelOptions) -> Vec<String> {
    let mut a = vec![
        "new".to_owned(),
        name.to_owned(),
        "--no-interaction".to_owned(),
    ];
    match &o.starter_kit {
        StarterKit::None => {}
        StarterKit::React => a.push("--react".to_owned()),
        StarterKit::Vue => a.push("--vue".to_owned()),
        StarterKit::Livewire => a.push("--livewire".to_owned()),
        StarterKit::Svelte => a.push("--svelte".to_owned()),
        StarterKit::Community(pkg) => {
            a.push("--using".to_owned());
            a.push(pkg.clone());
        }
    }
    if matches!(o.auth, AuthProvider::WorkOs) {
        a.push("--workos".to_owned());
    }
    if o.livewire_class_components {
        a.push("--livewire-class-components".to_owned());
    }
    if o.teams {
        a.push("--teams".to_owned());
    }
    match o.testing {
        Testing::Pest => a.push("--pest".to_owned()),
        Testing::PhpUnit => a.push("--phpunit".to_owned()),
    }
    a.push("--database".to_owned());
    a.push(database_flag(o.database).to_owned());
    match o.js {
        JsRuntime::Npm => a.push("--npm".to_owned()),
        JsRuntime::Bun => a.push("--bun".to_owned()),
        JsRuntime::Skip => a.push("--no-node".to_owned()),
    }
    if o.git {
        a.push("--git".to_owned());
    }
    // Pass an explicit boost flag either way so the installer never prompts.
    a.push(if o.boost { "--boost" } else { "--no-boost" }.to_owned());
    a
}

fn database_flag(d: Database) -> &'static str {
    match d {
        Database::Sqlite => "sqlite",
        Database::Mysql => "mysql",
        Database::Mariadb => "mariadb",
        Database::Pgsql => "pgsql",
        Database::Sqlsrv => "sqlsrv",
    }
}

/// Whether the installer will run `git` (any starter kit, or an explicit
/// `--git`).
fn needs_git(o: &LaravelOptions) -> bool {
    o.git || !matches!(o.starter_kit, StarterKit::None)
}

/// `{parent}/{name}` must not exist, or must be an empty directory.
fn check_target_dir(project_dir: &Path) -> Result<(), String> {
    match std::fs::read_dir(project_dir) {
        Ok(mut entries) => {
            if entries.next().is_some() {
                Err(format!(
                    "{} already exists and is not empty",
                    project_dir.display()
                ))
            } else {
                Ok(())
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("cannot read {}: {e}", project_dir.display())),
    }
}

/// Confirm the daemon can create files under `parent` (it may be any
/// user-chosen folder).
fn probe_writable(parent: &Path) -> Result<(), String> {
    if !parent.is_dir() {
        return Err(format!("{} is not a directory", parent.display()));
    }
    let probe = parent.join(format!(".yerd-write-probe-{}", std::process::id()));
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            Ok(())
        }
        Err(e) => Err(format!("cannot write to {}: {e}", parent.display())),
    }
}

/// Install Node/Bun if the chosen JS runtime needs it and it's absent.
async fn ensure_js_runtime(
    id: &str,
    js: JsRuntime,
    state: &Arc<DaemonState>,
) -> Result<(), String> {
    let tool = match js {
        JsRuntime::Npm => Tool::Node,
        JsRuntime::Bun => Tool::Bun,
        JsRuntime::Skip => return Ok(()),
    };
    if tools::installed_version(&state.dirs, tool).is_some() {
        return Ok(());
    }
    state
        .jobs
        .set_phase(id, format!("Installing {}", tool.display_name()))
        .await;
    let dl = crate::php_install::ReqwestDownloader::new();
    let guard = state.tool_mutate.lock().await;
    tools::install(tool, &state.dirs, &dl, None)
        .await
        .map_err(|e| format!("failed to install {}: {e}", tool.display_name()))?;
    drop(guard);
    crate::ipc_server::reconcile_tool_shims_now(state).await;
    Ok(())
}

/// Compose `PATH` = `<per-job bin> : <{data}/bin> : <inherited PATH>`.
fn composed_path(job_bin: &Path, data_bin: &Path) -> std::ffi::OsString {
    let mut entries = vec![job_bin.to_path_buf(), data_bin.to_path_buf()];
    if let Some(existing) = std::env::var_os("PATH") {
        entries.extend(std::env::split_paths(&existing));
    }
    std::env::join_paths(entries).unwrap_or_else(|_| std::ffi::OsString::from(job_bin))
}

/// `git --version` resolves on the composed PATH.
async fn git_available(path_env: &std::ffi::OsString) -> bool {
    tokio::process::Command::new("git")
        .arg("--version")
        .env("PATH", path_env)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .is_ok_and(|s| s.success())
}

/// Register the new project so the proxy serves it.
async fn register(
    name: &str,
    parent_dir: &Path,
    project_dir: &Path,
    spec: &CreateSiteSpec,
    state: &Arc<DaemonState>,
) -> Result<(), String> {
    let parent_canon =
        std::fs::canonicalize(parent_dir).unwrap_or_else(|_| parent_dir.to_path_buf());
    let (is_parked, default_php) = {
        let cfg = state.config.lock().await;
        let parked = cfg
            .parked
            .paths
            .contains(parent_canon.to_string_lossy().as_ref());
        (parked, cfg.php.default)
    };

    if is_parked {
        // Inside a parked root → the site auto-discovers; force a rescan/rebuild
        // via an idempotent re-Park (Park always rebuilds the router).
        mutate_ok(
            crate::ipc_server::handle_mutation(
                Request::Park {
                    path: parent_dir.to_path_buf(),
                },
                state,
            )
            .await,
        )?;
    } else {
        mutate_ok(
            crate::ipc_server::handle_mutation(
                Request::Link {
                    name: name.to_owned(),
                    path: project_dir.to_path_buf(),
                },
                state,
            )
            .await,
        )?;
    }

    // Apply non-default PHP / HTTPS (these also re-validate the now-present site).
    if spec.php != default_php {
        mutate_ok(
            crate::ipc_server::handle_mutation(
                Request::SetPhp {
                    name: name.to_owned(),
                    version: spec.php,
                },
                state,
            )
            .await,
        )?;
    }
    if spec.secure {
        mutate_ok(
            crate::ipc_server::handle_mutation(
                Request::SetSecure {
                    name: name.to_owned(),
                    secure: true,
                },
                state,
            )
            .await,
        )?;
    }
    Ok(())
}

/// Map a mutation `Response` to `Result`.
fn mutate_ok(resp: Response) -> Result<(), String> {
    match resp {
        Response::Ok => Ok(()),
        Response::Error { message, .. } => Err(message),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

/// Outcome of the streamed scaffold process.
enum ScaffoldOutcome {
    Ok,
    Failed(String),
    Cancelled,
}

/// Spawn `php <installer> new …`, stream both pipes into the job log, and wait —
/// killing the whole process group on cancel or timeout.
#[allow(clippy::too_many_arguments)]
async fn run_scaffold(
    id: &str,
    php_cli: &Path,
    installer_bin: &Path,
    args: &[String],
    cwd: &Path,
    path_env: &std::ffi::OsString,
    composer_home: &Path,
    state: &Arc<DaemonState>,
    cancel_rx: &mut watch::Receiver<bool>,
) -> ScaffoldOutcome {
    let mut cmd = tokio::process::Command::new(php_cli);
    cmd.arg(installer_bin)
        .args(args)
        .current_dir(cwd)
        .env("PATH", path_env)
        .env("COMPOSER_HOME", composer_home)
        .env("COMPOSER_NO_INTERACTION", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // New process group so cancel/timeout can signal the installer's children
    // (composer, npm, git), not just the `php` parent.
    #[cfg(unix)]
    cmd.process_group(0);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return ScaffoldOutcome::Failed(format!("failed to start laravel installer: {e}"))
        }
    };
    let pgid = child.id();

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let mut readers = Vec::new();
    if let Some(out) = stdout {
        readers.push(spawn_reader(state.clone(), id.to_owned(), out));
    }
    if let Some(err) = stderr {
        readers.push(spawn_reader(state.clone(), id.to_owned(), err));
    }

    let result = tokio::select! {
        changed = cancel_rx.changed() => {
            // `changed()` resolves on the cancel signal; if the sender is gone we
            // also stop. Either way, tear the process down.
            let cancelled = changed.is_ok() && *cancel_rx.borrow();
            // Abort the pipe readers first: closing the read ends makes any
            // grandchild blocked on a full pipe get SIGPIPE, so the group dies
            // promptly instead of stalling the SIGTERM grace window.
            for r in &readers { r.abort(); }
            terminate_group(pgid, &mut child).await;
            if cancelled { ScaffoldOutcome::Cancelled } else { ScaffoldOutcome::Failed("cancel channel closed".to_owned()) }
        }
        timed = tokio::time::timeout(SCAFFOLD_TIMEOUT, child.wait()) => {
            match timed {
                Ok(Ok(status)) if status.success() => ScaffoldOutcome::Ok,
                Ok(Ok(status)) => ScaffoldOutcome::Failed(format!("laravel new exited with {status}")),
                Ok(Err(e)) => ScaffoldOutcome::Failed(format!("waiting for laravel new: {e}")),
                Err(_) => {
                    for r in &readers { r.abort(); }
                    terminate_group(pgid, &mut child).await;
                    ScaffoldOutcome::Failed("laravel new timed out".to_owned())
                }
            }
        }
    };

    // Let the readers drain any buffered output before returning.
    for r in readers {
        let _ = r.await;
    }
    result
}

/// Read a child pipe line-by-line into the job log.
fn spawn_reader<R>(state: Arc<DaemonState>, id: String, pipe: R) -> tokio::task::JoinHandle<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(pipe).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            state.jobs.push_log(&id, line).await;
        }
    })
}

/// Send SIGTERM to the process group, then SIGKILL after a short grace.
async fn terminate_group(pgid: Option<u32>, child: &mut tokio::process::Child) {
    #[cfg(unix)]
    {
        kill_group(pgid, nix::sys::signal::Signal::SIGTERM);
        if tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .is_err()
        {
            kill_group(pgid, nix::sys::signal::Signal::SIGKILL);
            let _ = child.wait().await;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = pgid;
        let _ = child.start_kill();
        let _ = child.wait().await;
    }
}

#[cfg(unix)]
fn kill_group(pgid: Option<u32>, signal: nix::sys::signal::Signal) {
    if let Some(p) = pgid {
        // The child is the group leader (pgid == pid) because we set
        // `process_group(0)` at spawn.
        if let Ok(pid) = i32::try_from(p) {
            let _ = nix::sys::signal::killpg(nix::unistd::Pid::from_raw(pid), signal);
        }
    }
}

/// Single-quote a path for safe inclusion in a `/bin/sh` script, escaping any
/// embedded single quotes (`'` → `'\''`). Without this a data dir containing a
/// `'` (e.g. `/Users/o'brien/…`) would produce a broken wrapper script.
#[cfg(unix)]
fn sh_quote(p: &Path) -> String {
    format!("'{}'", p.to_string_lossy().replace('\'', "'\\''"))
}

/// Build `{job_dir}/bin` containing a `php` symlink to the chosen version and a
/// `composer` wrapper that runs that same PHP, so the installer's nested
/// `composer create-project` uses the requested runtime (Composer derives its
/// child PHP from `PHP_BINARY`). Unix-only.
#[cfg(unix)]
fn build_job_bin(job_dir: &Path, php_cli: &Path, composer_phar: &Path) -> Result<PathBuf, String> {
    use std::os::unix::fs::PermissionsExt;

    let bin = job_dir.join("bin");
    std::fs::create_dir_all(&bin).map_err(|e| format!("{}: {e}", bin.display()))?;

    let php_link = bin.join("php");
    let _ = std::fs::remove_file(&php_link);
    std::os::unix::fs::symlink(php_cli, &php_link).map_err(|e| format!("link php: {e}"))?;

    let composer = bin.join("composer");
    let script = format!(
        "#!/bin/sh\nexec {} {} \"$@\"\n",
        sh_quote(php_cli),
        sh_quote(composer_phar)
    );
    std::fs::write(&composer, script).map_err(|e| format!("write composer wrapper: {e}"))?;
    std::fs::set_permissions(&composer, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| format!("chmod composer wrapper: {e}"))?;
    Ok(bin)
}

#[cfg(not(unix))]
fn build_job_bin(
    _job_dir: &Path,
    _php_cli: &Path,
    _composer_phar: &Path,
) -> Result<PathBuf, String> {
    Err("site creation is not yet supported on this platform".to_owned())
}

fn error(code: ErrorCode, message: String) -> Response {
    Response::Error { code, message }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use yerd_ipc::{AuthProvider, Database, JsRuntime, LaravelOptions, StarterKit, Testing};

    fn opts() -> LaravelOptions {
        LaravelOptions {
            starter_kit: StarterKit::None,
            auth: AuthProvider::Laravel,
            livewire_class_components: false,
            teams: false,
            testing: Testing::Pest,
            database: Database::Sqlite,
            js: JsRuntime::Skip,
            git: false,
            boost: false,
        }
    }

    #[test]
    fn minimal_args() {
        let a = build_new_args("blog", &opts());
        assert_eq!(
            a,
            vec![
                "new",
                "blog",
                "--no-interaction",
                "--pest",
                "--database",
                "sqlite",
                "--no-node",
                "--no-boost",
            ]
        );
    }

    #[test]
    fn react_pest_sqlite_npm_git_args() {
        let mut o = opts();
        o.starter_kit = StarterKit::React;
        o.js = JsRuntime::Npm;
        o.git = true;
        let a = build_new_args("shop", &o);
        assert_eq!(
            a,
            vec![
                "new",
                "shop",
                "--no-interaction",
                "--react",
                "--pest",
                "--database",
                "sqlite",
                "--npm",
                "--git",
                "--no-boost",
            ]
        );
    }

    #[test]
    fn livewire_workos_teams_phpunit_pgsql_bun_boost_args() {
        let o = LaravelOptions {
            starter_kit: StarterKit::Livewire,
            auth: AuthProvider::WorkOs,
            livewire_class_components: true,
            teams: true,
            testing: Testing::PhpUnit,
            database: Database::Pgsql,
            js: JsRuntime::Bun,
            git: false,
            boost: true,
        };
        let a = build_new_args("crm", &o);
        assert_eq!(
            a,
            vec![
                "new",
                "crm",
                "--no-interaction",
                "--livewire",
                "--workos",
                "--livewire-class-components",
                "--teams",
                "--phpunit",
                "--database",
                "pgsql",
                "--bun",
                "--boost",
            ]
        );
    }

    #[test]
    fn community_kit_uses_using_with_package() {
        let mut o = opts();
        o.starter_kit = StarterKit::Community("acme/kit".to_owned());
        let a = build_new_args("x", &o);
        assert!(a.windows(2).any(|w| w == ["--using", "acme/kit"]));
    }

    #[test]
    fn needs_git_for_kits_and_explicit_flag() {
        let mut o = opts();
        assert!(!needs_git(&o));
        o.git = true;
        assert!(needs_git(&o));
        o.git = false;
        o.starter_kit = StarterKit::Vue;
        assert!(needs_git(&o));
    }

    #[test]
    fn check_target_dir_accepts_absent_and_empty() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(check_target_dir(&tmp.path().join("nope")).is_ok());
        let empty = tmp.path().join("empty");
        std::fs::create_dir(&empty).unwrap();
        assert!(check_target_dir(&empty).is_ok());
        std::fs::write(empty.join("f"), b"x").unwrap();
        assert!(check_target_dir(&empty).is_err());
    }
}
