//! `CreateSite` - scaffold a new project (`laravel new` or WP-CLI) then
//! register it.
//!
//! Scaffolding runs far longer than one request/response round-trip and streams
//! output, so this runs as a background [job](crate::jobs): [`start`] spawns the
//! work and returns a [`Response::JobStarted`] immediately; the client polls
//! `JobStatus` for the streamed log + phase.
//!
//! This module holds the framework-agnostic job orchestration (name
//! reservation, the per-job scratch dir, `JobRegistry` wiring, the streamed-
//! process runner, registration); [`laravel`] and [`wordpress`] hold each
//! framework's own scaffolding body, dispatched on `spec.framework` from
//! [`run_inner`].

mod laravel;
mod registration;
mod wordpress;

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::watch;

use yerd_ipc::{CreateSiteSpec, ErrorCode, Framework, JobState, Response};

use crate::state::DaemonState;
use crate::tools::{self, Tool};

/// Hard cap on a single scaffolding/WP-CLI step (Composer + optional `npm
/// install && build`, or one `wp core …` invocation). Hitting it kills the
/// process group and fails the job.
const STEP_TIMEOUT: Duration = Duration::from_secs(20 * 60);

/// Validate the request synchronously, then spawn the background job.
pub async fn start(spec: CreateSiteSpec, state: Arc<DaemonState>) -> Response {
    // The enum is `#[non_exhaustive]`, so a catch-all is required and guards
    // future variants.
    match &spec.framework {
        Framework::Laravel { .. } | Framework::Wordpress { .. } => {}
        _ => return error(ErrorCode::Internal, "unsupported framework".to_owned()),
    }

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

/// Dispatch to the framework-specific scaffolding body.
async fn run_inner(
    id: &str,
    name: &str,
    spec: &CreateSiteSpec,
    job_dir: &Path,
    state: &Arc<DaemonState>,
    cancel_rx: watch::Receiver<bool>,
) -> Outcome {
    match &spec.framework {
        Framework::Laravel { options } => {
            laravel::run(id, name, spec, options, job_dir, state, cancel_rx).await
        }
        Framework::Wordpress { options } => {
            wordpress::run(id, name, spec, options, state, cancel_rx).await
        }
        _ => Outcome::Failed("unsupported framework".to_owned()),
    }
}

/// Whether cancellation has been requested - a cheap, non-blocking poll (does
/// not consume the "changed" flag `tokio::select!` would). Used between
/// sequential steps that aren't themselves wrapped in a `select!` (e.g.
/// database provisioning), where "stop before the next step" is the
/// achievable granularity rather than "kill instantly".
fn is_cancelled(cancel_rx: &watch::Receiver<bool>) -> bool {
    *cancel_rx.borrow()
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

/// Install `tool` inline if it's neither managed nor available externally on
/// the user's PATH, streaming an `Installing {display_name}` phase update
/// into this job's log. Shared by both frameworks' Preflight steps (Laravel's
/// optional Node/Bun, WordPress's required WP-CLI).
async fn ensure_tool(
    id: &str,
    tool: Tool,
    user_dirs: &[PathBuf],
    state: &Arc<DaemonState>,
) -> Result<(), String> {
    if tools::installed_version(&state.dirs, tool).is_some() {
        return Ok(());
    }
    let data_bin = tools::bin_dir(&state.dirs);
    if crate::tools::external::external_tool(user_dirs, tool, &data_bin, &state.dirs.data).is_some()
    {
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

/// Outcome of a streamed subprocess run via [`run_streamed`].
enum StreamedOutcome {
    Ok,
    Failed(String),
    Cancelled,
}

/// Spawn `php <entry_point> <args…>`, stream both pipes into the job log, and
/// wait - killing the whole process group on cancel or timeout. Shared by
/// Laravel's `laravel new` (with a per-job `PATH`/`COMPOSER_HOME` so its
/// nested `composer create-project` uses the right runtime) and WordPress's
/// `wp core download`/`config create`/`core install` (which need neither,
/// since WP-CLI's own subcommands don't shell out to Composer or rely on
/// PATH-resolved tools).
///
/// Forces `NO_COLOR=1` + `TERM=dumb` on the child: stdout/stderr are pipes (not a
/// tty), but Symfony Console / Laravel Prompts / WP-CLI still emit ANSI colour
/// and cursor-control (spinner redraws) from the inherited TERM. The job log is
/// shown in a plain `<pre>` with no terminal emulator, so those escapes render
/// literally and spinner frames stack as duplicate lines; the two env vars push
/// most tools toward undecorated, single-line output, and
/// `crate::jobs::JobRegistry::push_log` strips whatever colour/cursor escapes
/// still get through (some spinners write them unconditionally).
#[allow(clippy::too_many_arguments)]
async fn run_streamed(
    id: &str,
    php_cli: &Path,
    php_flags: &[String],
    entry_point: &Path,
    args: &[String],
    cwd: &Path,
    path_env: Option<&std::ffi::OsString>,
    composer_home: Option<&Path>,
    state: &Arc<DaemonState>,
    cancel_rx: &mut watch::Receiver<bool>,
) -> StreamedOutcome {
    let mut cmd = tokio::process::Command::new(php_cli);
    cmd.args(php_flags)
        .arg(entry_point)
        .args(args)
        .current_dir(cwd)
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(p) = path_env {
        cmd.env("PATH", p);
    }
    if let Some(home) = composer_home {
        cmd.env("COMPOSER_HOME", home)
            .env("COMPOSER_NO_INTERACTION", "1");
    }
    #[cfg(unix)]
    cmd.process_group(0);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return StreamedOutcome::Failed(format!(
                "failed to start {}: {e}",
                entry_point.display()
            ))
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
            let cancelled = changed.is_ok() && *cancel_rx.borrow();
            for r in &readers { r.abort(); }
            terminate_group(pgid, &mut child).await;
            if cancelled { StreamedOutcome::Cancelled } else { StreamedOutcome::Failed("cancel channel closed".to_owned()) }
        }
        timed = tokio::time::timeout(STEP_TIMEOUT, child.wait()) => {
            match timed {
                Ok(Ok(status)) if status.success() => StreamedOutcome::Ok,
                Ok(Ok(status)) => StreamedOutcome::Failed(format!("{} exited with {status}", entry_point.display())),
                Ok(Err(e)) => StreamedOutcome::Failed(format!("waiting for {}: {e}", entry_point.display())),
                Err(_) => {
                    for r in &readers { r.abort(); }
                    terminate_group(pgid, &mut child).await;
                    StreamedOutcome::Failed(format!("{} timed out", entry_point.display()))
                }
            }
        }
    };

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
        if let Ok(pid) = i32::try_from(p) {
            let _ = nix::sys::signal::killpg(nix::unistd::Pid::from_raw(pid), signal);
        }
    }
}

fn error(code: ErrorCode, message: String) -> Response {
    Response::Error { code, message }
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
    use tokio::sync::{Mutex, RwLock};
    use yerd_core::{PhpVersion, RouterConfig, SiteRouter, Tld};
    use yerd_ipc::{
        AuthProvider, Database, JsRuntime, LaravelOptions, StarterKit, Testing, WordPressDatabase,
        WordPressDbEngine, WordPressOptions,
    };
    use yerd_platform::PlatformDirs;

    fn dirs_in(tmp: &Path) -> PlatformDirs {
        PlatformDirs {
            config: tmp.join("c"),
            data: tmp.join("d"),
            state: tmp.join("s"),
            cache: tmp.join("ca"),
            runtime: tmp.join("r"),
        }
    }

    /// Copied verbatim from `ipc_server`'s test module (its `state_in` is private
    /// to that module). A `DaemonState` rooted at `tmp`.
    fn state_in(tmp: &Path) -> DaemonState {
        let dirs = dirs_in(tmp);
        let router = SiteRouter::new(RouterConfig::with_tld(Tld::new("test").unwrap()));
        let ca_path = dirs.data.join("ca.cert.pem");
        let php_manager = Arc::new(Mutex::new(yerd_php::PhpManager::new(
            yerd_php::TokioProcessSpawner,
            yerd_php::SystemClock,
            yerd_php::io::FastCgiProbe,
            dirs.clone(),
            yerd_platform::ActivePortBinder::new(),
            std::process::id(),
            std::collections::BTreeMap::new(),
        )));
        DaemonState {
            config: Mutex::new(yerd_config::Config::default()),
            router: Arc::new(RwLock::new(router)),
            config_path: dirs.config.join("yerd.toml"),
            dirs,
            dns_addr: "127.0.0.1:1053".parse().unwrap(),
            ca_path,
            ca_fingerprint: yerd_platform::CaFingerprint::new([0u8; 32]),
            php_ca_bundle: None,
            php_updates: tokio::sync::RwLock::new(std::collections::HashMap::new()),
            yerd_update: tokio::sync::RwLock::new(Vec::new()),
            update_snapshot: tokio::sync::RwLock::new(None),
            php_manager,
            service_manager: Arc::new(Mutex::new(crate::services::new_manager(dirs_in(tmp)))),
            mail_store: Arc::new(yerd_mail::Store::open(tmp.join("mail")).unwrap()),
            mail: crate::state::MailRuntime { listening: false },
            http: yerd_ipc::PortStatus {
                requested: 80,
                bound: 8080,
                fell_back: true,
            },
            https: yerd_ipc::PortStatus {
                requested: 443,
                bound: 8443,
                fell_back: true,
            },
            redirect_https_port: std::sync::Arc::new(std::sync::atomic::AtomicU16::new(8443)),
            web_unbound: None,
            dns_unbound: None,
            boot_id: 1,
            started_at: std::time::Instant::now(),
            shutdown_tx: tokio::sync::watch::channel(false).0,
            restart_requested: std::sync::atomic::AtomicBool::new(false),
            detect_cache: Arc::new(crate::detect_cache::DetectCache::new()),
            watch_dirty: tokio::sync::Notify::new(),
            dumps: Arc::new(crate::dump_server::DumpStore::new()),
            shim_reconcile: tokio::sync::Mutex::new(()),
            tunnel_manager: std::sync::Arc::new(tokio::sync::Mutex::new(
                crate::tunnel::new_manager(),
            )),
            cloudflared_resolution: tokio::sync::RwLock::new(None),
            tool_mutate: tokio::sync::Mutex::new(()),
            tunnel_mutate: tokio::sync::Mutex::new(()),
            php_mutate: tokio::sync::Mutex::new(()),
            jobs: crate::jobs::JobRegistry::default(),
            reserved_names: tokio::sync::Mutex::new(std::collections::HashSet::new()),
            wordpress_versions: tokio::sync::RwLock::new(None),
            wordpress_login_tokens: Arc::new(crate::wordpress_login::LoginTokenRegistry::new()),
            wordpress_login_prepend_script: None,
            wordpress_sites: std::sync::Arc::new(tokio::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
        }
    }

    fn laravel_opts() -> LaravelOptions {
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

    fn wordpress_opts() -> WordPressOptions {
        WordPressOptions {
            core_version: None,
            locale: "en_GB".to_owned(),
            admin_user: "admin".to_owned(),
            admin_email: "admin@blog.test".to_owned(),
            admin_password: "hunter2hunter2".to_owned(),
            site_title: "My Blog".to_owned(),
            table_prefix: "wp_".to_owned(),
            database: WordPressDatabase {
                engine: WordPressDbEngine::Mysql,
                name: "blog".to_owned(),
            },
        }
    }

    fn laravel_spec(name: &str, parent: &Path) -> CreateSiteSpec {
        CreateSiteSpec {
            name: name.to_owned(),
            parent_dir: parent.to_path_buf(),
            php: PhpVersion::new(8, 3),
            secure: false,
            framework: Framework::Laravel {
                options: laravel_opts(),
            },
        }
    }

    fn wordpress_spec(name: &str, parent: &Path) -> CreateSiteSpec {
        CreateSiteSpec {
            name: name.to_owned(),
            parent_dir: parent.to_path_buf(),
            php: PhpVersion::new(8, 3),
            secure: true,
            framework: Framework::Wordpress {
                options: wordpress_opts(),
            },
        }
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

    #[test]
    fn check_target_dir_errors_on_unreadable_path() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("afile");
        std::fs::write(&f, b"x").unwrap();
        let err = check_target_dir(&f).unwrap_err();
        assert!(err.contains("cannot read"), "got {err:?}");
    }

    #[test]
    fn probe_writable_accepts_dir_and_rejects_non_dir() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(probe_writable(tmp.path()).is_ok());
        let leftovers: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains("yerd-write-probe"))
            .collect();
        assert!(leftovers.is_empty(), "probe file should be removed");

        let f = tmp.path().join("afile");
        std::fs::write(&f, b"x").unwrap();
        assert!(probe_writable(&f).is_err());
        assert!(probe_writable(&tmp.path().join("missing")).is_err());
    }

    #[test]
    fn error_builds_response_error() {
        match error(ErrorCode::InvalidPath, "nope".to_owned()) {
            Response::Error { code, message } => {
                assert_eq!(code, ErrorCode::InvalidPath);
                assert_eq!(message, "nope");
            }
            other => panic!("expected error, got {other:?}"),
        }
    }

    #[test]
    fn is_cancelled_reflects_current_value() {
        let (tx, rx) = watch::channel(false);
        assert!(!is_cancelled(&rx));
        let _ = tx.send(true);
        assert!(is_cancelled(&rx));
    }

    #[tokio::test]
    async fn start_rejects_invalid_site_name() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(state_in(tmp.path()));
        let spec = laravel_spec("Not A Valid Name!", tmp.path());
        match start(spec, state).await {
            Response::Error { code, .. } => assert_eq!(code, ErrorCode::InvalidPath),
            other => panic!("expected error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn start_accepts_wordpress_framework() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(state_in(tmp.path()));
        let spec = wordpress_spec("blog", tmp.path());
        match start(spec, state).await {
            Response::JobStarted { .. } => {}
            other => panic!("expected JobStarted, got {other:?}"),
        }
    }
}
