//! Database administration ("Manage DBs"): list / create / drop databases in a
//! running SQL service.
//!
//! This is the I/O edge - it shells out to the **bundled client** for each
//! engine (`mysql`/`mariadb` over the Unix socket; `psql` over TCP loopback) and
//! captures its output. All the decision logic - name validation, SQL and `argv`
//! construction, output parsing - is pure and unit-tested in
//! `yerd_services::database`. The SQL is passed as a single `argv` element (never
//! a shell), so combined with the validating allowlist there is no injection
//! surface.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use yerd_ipc::{DatabaseSummary, ErrorCode, Response};
use yerd_services::{database, version, Service, ServiceRunState};

use crate::services::resolve_version;
use crate::state::DaemonState;

/// `list databases <svc>` - the user databases (system schemas filtered out).
pub async fn list(service_id: &str, state: &DaemonState) -> Response {
    let ctx = match prepare(service_id, state).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    match run_client(&ctx, database::list_sql(ctx.service)).await {
        Ok(stdout) => Response::Databases {
            databases: database::parse_db_list(ctx.service, &stdout)
                .into_iter()
                .map(|name| DatabaseSummary { name })
                .collect(),
        },
        Err(r) => r,
    }
}

/// `create database <svc> <name>`.
pub async fn create(service_id: &str, name: &str, state: &DaemonState) -> Response {
    let ctx = match prepare(service_id, state).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    if let Err(e) = database::validate_db_name(name) {
        return invalid_name(&e.to_string());
    }
    match run_client(&ctx, &database::create_sql(ctx.service, name)).await {
        Ok(_) => Response::Ok,
        Err(r) => r,
    }
}

/// `drop database <svc> <name>` - refuses system databases.
pub async fn drop(service_id: &str, name: &str, state: &DaemonState) -> Response {
    let ctx = match prepare(service_id, state).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    if let Err(e) = database::validate_db_name(name) {
        return invalid_name(&e.to_string());
    }
    if database::is_system_database(ctx.service, name) {
        return Response::Error {
            code: ErrorCode::InvalidPath,
            message: format!("refusing to drop the system database {name:?}"),
        };
    }
    match run_client(&ctx, &database::drop_sql(ctx.service, name)).await {
        Ok(_) => Response::Ok,
        Err(r) => r,
    }
}

/// `backup database <svc> <name> <path>` - stream a plain-SQL dump to `path`.
///
/// The dump tool writes to stdout; we stream that to a temp sibling of `path` and
/// atomically rename on success, so a failed dump never truncates an existing target.
/// The destination path is never passed to the dump tool - there is no path-injection
/// surface.
pub async fn backup(service_id: &str, name: &str, path: &Path, state: &DaemonState) -> Response {
    let ctx = match prepare(service_id, state).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    if let Err(e) = database::validate_db_name(name) {
        return invalid_name(&e.to_string());
    }
    let Some(dump_bin) = ctx.service.dump_binary() else {
        return invalid_path(format!(
            "{} does not support backups",
            ctx.service.display_name()
        ));
    };
    let dump_path = ctx.bin_dir.join(dump_bin);
    if !dump_path.is_file() {
        return internal(format!(
            "this {} build does not include {dump_bin}",
            ctx.service.display_name()
        ));
    }
    let args = database::dump_args(ctx.service, &ctx.socket, ctx.port, name);

    let tmp = tmp_sibling(path);
    let mut file = match tokio::fs::File::create(&tmp).await {
        Ok(f) => f,
        Err(e) => return internal(format!("create {}: {e}", tmp.display())),
    };

    let mut child = match Command::new(&dump_path)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            let _ = tokio::fs::remove_file(&tmp).await;
            return internal(format!("run {}: {e}", dump_path.display()));
        }
    };

    let (Some(mut stdout), Some(mut stderr)) = (child.stdout.take(), child.stderr.take()) else {
        let _ = tokio::fs::remove_file(&tmp).await;
        return internal("dump tool produced no stdio pipes");
    };
    let copy = async move {
        let res = tokio::io::copy(&mut stdout, &mut file).await;
        if res.is_ok() {
            let _ = file.flush().await;
        }
        res
    };
    let (copy_res, stderr_buf, wait_res) = tokio::join!(copy, read_all(&mut stderr), child.wait());

    let fail = |resp: Response| async {
        let _ = tokio::fs::remove_file(&tmp).await;
        resp
    };
    let status = match wait_res {
        Ok(s) => s,
        Err(e) => return fail(internal(format!("await {dump_bin}: {e}"))).await,
    };
    if let Err(e) = copy_res {
        return fail(internal(format!("write {}: {e}", tmp.display()))).await;
    }
    if !status.success() {
        return fail(map_stderr(&stderr_buf)).await;
    }
    if let Err(e) = tokio::fs::rename(&tmp, path).await {
        return fail(internal(format!("finalise {}: {e}", path.display()))).await;
    }
    Response::Ok
}

/// `restore database <svc> <name> <path>` - replay a plain-SQL file into `name`.
///
/// The file is streamed into the restore client's stdin (the source path never
/// reaches its argv). The target database must already exist (single-db dumps carry
/// no `CREATE DATABASE`); a missing target surfaces as the engine's own error.
pub async fn restore(service_id: &str, name: &str, path: &Path, state: &DaemonState) -> Response {
    let ctx = match prepare(service_id, state).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    if let Err(e) = database::validate_db_name(name) {
        return invalid_name(&e.to_string());
    }
    if database::is_system_database(ctx.service, name) {
        return invalid_path(format!(
            "refusing to restore over the system database {name:?}"
        ));
    }
    let mut file = match tokio::fs::File::open(path).await {
        Ok(f) => f,
        Err(e) => return invalid_path(format!("open {}: {e}", path.display())),
    };
    let args = database::restore_args(ctx.service, &ctx.socket, ctx.port, name);

    let mut child = match Command::new(&ctx.client_path)
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return internal(format!("run {}: {e}", ctx.client_path.display())),
    };

    let (Some(mut stdin), Some(mut stderr)) = (child.stdin.take(), child.stderr.take()) else {
        return internal("restore client produced no stdio pipes");
    };
    let feed = async move {
        let res = tokio::io::copy(&mut file, &mut stdin).await;
        let _ = stdin.shutdown().await;
        res
    };
    let (feed_res, stderr_buf, wait_res) = tokio::join!(feed, read_all(&mut stderr), child.wait());

    let status = match wait_res {
        Ok(s) => s,
        Err(e) => return internal(format!("await restore: {e}")),
    };
    if let Err(e) = feed_res {
        return internal(format!("feed {}: {e}", path.display()));
    }
    if !status.success() {
        return map_stderr(&stderr_buf);
    }
    Response::Ok
}

// ── internals ────────────────────────────────────────────────────────────────

/// Everything `run_client` needs, resolved once per request.
struct DbCtx {
    service: Service,
    /// The install's `bin/` dir, so backup can also resolve the dump binary.
    bin_dir: PathBuf,
    client_path: PathBuf,
    socket: PathBuf,
    port: u16,
}

/// Resolve the service, verify it's a running SQL engine with a usable client
/// binary, and gather the connection details. Holds the config / service-manager
/// locks only briefly and never across the (later) client spawn.
async fn prepare(service_id: &str, state: &DaemonState) -> Result<DbCtx, Response> {
    let Some(service) = Service::from_id(service_id) else {
        return Err(Response::Error {
            code: ErrorCode::NotFound,
            message: format!("unknown service {service_id:?}"),
        });
    };
    let Some(client) = service.client_binary() else {
        return Err(Response::Error {
            code: ErrorCode::InvalidPath,
            message: format!("{} does not host SQL databases", service.display_name()),
        });
    };

    let configured = {
        let cfg = state.config.lock().await;
        cfg.services
            .instances
            .get(service.id())
            .and_then(|i| i.version.clone())
    };
    let ver = resolve_version(service, configured.as_deref(), &state.dirs)?;
    let bin_dir = version::install_dir(&state.dirs, service, &ver).join("bin");
    let client_path = bin_dir.join(client);
    if !client_path.is_file() {
        return Err(Response::Error {
            code: ErrorCode::Internal,
            message: format!(
                "this {} build does not include {client}",
                service.display_name()
            ),
        });
    }

    let running = {
        let mut mgr = state.service_manager.lock().await;
        mgr.snapshots()
            .into_iter()
            .any(|s| s.service == service && s.state == ServiceRunState::Running)
    };
    if !running {
        return Err(Response::Error {
            code: ErrorCode::Internal,
            message: format!(
                "start {} first - managing databases needs a running server",
                service.id()
            ),
        });
    }

    let port = {
        let cfg = state.config.lock().await;
        cfg.services
            .instances
            .get(service.id())
            .and_then(|i| i.port)
            .unwrap_or(service.default_port())
    };
    Ok(DbCtx {
        service,
        bin_dir,
        client_path,
        socket: version::socket_path(&state.dirs, service),
        port,
    })
}

/// Run one SQL statement through the bundled client, returning stdout on a clean
/// exit or a mapped [`Response::Error`] otherwise.
async fn run_client(ctx: &DbCtx, sql: &str) -> Result<String, Response> {
    let args = database::client_args(ctx.service, &ctx.socket, ctx.port, sql);
    let output = Command::new(&ctx.client_path)
        .args(&args)
        .output()
        .await
        .map_err(|e| internal(format!("run {}: {e}", ctx.client_path.display())))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    Err(map_stderr(&output.stderr))
}

/// Read an async reader fully into a buffer, ignoring read errors (best-effort
/// capture of a child's stderr for diagnostics).
async fn read_all<R: AsyncReadExt + Unpin>(reader: &mut R) -> Vec<u8> {
    let mut buf = Vec::new();
    let _ = reader.read_to_end(&mut buf).await;
    buf
}

/// A sibling temp path for atomic backup writes (`<name>.yerd-part`).
fn tmp_sibling(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(std::ffi::OsStr::to_os_string)
        .unwrap_or_default();
    name.push(".yerd-part");
    path.with_file_name(name)
}

/// Map a child's stderr bytes to a typed [`Response::Error`], keying the code off the
/// first non-empty line (the message preserves the engine's exact wording).
fn map_stderr(stderr: &[u8]) -> Response {
    let stderr = String::from_utf8_lossy(stderr);
    let detail = stderr
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("the database client exited with an error")
        .to_owned();
    Response::Error {
        code: classify(&detail),
        message: detail,
    }
}

/// A daemon-side failure that doesn't fit a typed code.
fn internal(message: impl Into<String>) -> Response {
    Response::Error {
        code: ErrorCode::Internal,
        message: message.into(),
    }
}

/// A rejected path / unsupported operation.
fn invalid_path(message: impl Into<String>) -> Response {
    Response::Error {
        code: ErrorCode::InvalidPath,
        message: message.into(),
    }
}

/// Best-effort mapping of a client error line to a typed code (the message still
/// carries the engine's exact wording).
fn classify(detail: &str) -> ErrorCode {
    let d = detail.to_ascii_lowercase();
    if d.contains("already exists") || d.contains("database exists") {
        ErrorCode::AlreadyExists
    } else if d.contains("does not exist")
        || d.contains("doesn't exist")
        || d.contains("unknown database")
    {
        ErrorCode::NotFound
    } else {
        ErrorCode::Internal
    }
}

/// A name that failed [`database::validate_db_name`].
fn invalid_name(detail: &str) -> Response {
    Response::Error {
        code: ErrorCode::InvalidPath,
        message: detail.to_owned(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::{Mutex, RwLock};
    use yerd_core::{RouterConfig, SiteRouter, Tld};
    use yerd_platform::PlatformDirs;

    /// Pull `(code, message)` out of a `Response::Error`, panicking otherwise.
    fn err_parts(r: Response) -> (ErrorCode, String) {
        match r {
            Response::Error { code, message } => (code, message),
            other => panic!("expected Response::Error, got {other:?}"),
        }
    }

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
    /// to that module). A `DaemonState` rooted at `tmp` with no engines installed.
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
            tool_mutate: tokio::sync::Mutex::new(()),
            tunnel_mutate: tokio::sync::Mutex::new(()),
            php_mutate: tokio::sync::Mutex::new(()),
            jobs: crate::jobs::JobRegistry::default(),
            reserved_names: tokio::sync::Mutex::new(std::collections::HashSet::new()),
        }
    }

    #[tokio::test]
    async fn prepare_rejects_unknown_service() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let (code, msg) = err_parts(list("nonsense", &state).await);
        assert_eq!(code, ErrorCode::NotFound);
        assert!(msg.contains("nonsense"));
    }

    #[tokio::test]
    async fn prepare_rejects_non_sql_engine() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let (code, msg) = err_parts(list("redis", &state).await);
        assert_eq!(code, ErrorCode::InvalidPath);
        assert!(msg.contains("does not host SQL databases"));
    }

    #[tokio::test]
    async fn prepare_errors_when_engine_not_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let (code, _) = err_parts(list("mysql", &state).await);
        assert_eq!(code, ErrorCode::NotFound);
    }

    #[tokio::test]
    async fn every_db_op_propagates_prepare_error() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let p = std::path::Path::new("/tmp/x.sql");
        assert_eq!(
            err_parts(create("mysql", "blog", &state).await).0,
            ErrorCode::NotFound
        );
        assert_eq!(
            err_parts(drop("mysql", "blog", &state).await).0,
            ErrorCode::NotFound
        );
        assert_eq!(
            err_parts(backup("mysql", "blog", p, &state).await).0,
            ErrorCode::NotFound
        );
        assert_eq!(
            err_parts(restore("mysql", "blog", p, &state).await).0,
            ErrorCode::NotFound
        );
    }

    #[test]
    fn classify_maps_already_exists() {
        assert_eq!(
            classify("ERROR 1007: Can't create database 'x'; database exists"),
            ErrorCode::AlreadyExists,
            "MySQL's duplicate-database phrasing maps to AlreadyExists"
        );
        assert_eq!(
            classify("database \"foo\" already exists"),
            ErrorCode::AlreadyExists
        );
        assert_eq!(classify("Already Exists"), ErrorCode::AlreadyExists);
    }

    #[test]
    fn classify_maps_not_found_variants() {
        assert_eq!(
            classify("database \"foo\" does not exist"),
            ErrorCode::NotFound
        );
        assert_eq!(
            classify("ERROR 1008: Can't drop database 'x'; doesn't exist"),
            ErrorCode::NotFound
        );
        assert_eq!(classify("Unknown database 'bar'"), ErrorCode::NotFound);
    }

    #[test]
    fn classify_defaults_to_internal() {
        assert_eq!(classify("some other failure"), ErrorCode::Internal);
        assert_eq!(classify(""), ErrorCode::Internal);
    }

    #[test]
    fn tmp_sibling_appends_part_suffix() {
        let p = Path::new("/tmp/dumps/blog.sql");
        let sib = tmp_sibling(p);
        assert_eq!(sib, Path::new("/tmp/dumps/blog.sql.yerd-part"));
        assert_eq!(sib.parent(), p.parent());
    }

    #[test]
    fn tmp_sibling_handles_pathless_name() {
        let sib = tmp_sibling(Path::new("/"));
        assert!(sib.to_string_lossy().ends_with(".yerd-part"));
    }

    #[test]
    fn map_stderr_uses_first_non_empty_trimmed_line() {
        let (code, msg) = err_parts(map_stderr(
            b"\n   \n  ERROR: database \"x\" already exists  \nignored second line\n",
        ));
        assert_eq!(msg, "ERROR: database \"x\" already exists");
        assert_eq!(code, ErrorCode::AlreadyExists);
    }

    #[test]
    fn map_stderr_empty_falls_back_to_default_message() {
        let (code, msg) = err_parts(map_stderr(b""));
        assert_eq!(msg, "the database client exited with an error");
        assert_eq!(code, ErrorCode::Internal);
    }

    #[test]
    fn map_stderr_lossy_decodes_invalid_utf8() {
        let (_code, msg) = err_parts(map_stderr(&[0xff, 0xfe, b'\n']));
        assert!(!msg.is_empty());
    }

    #[test]
    fn constructors_set_expected_codes() {
        assert_eq!(err_parts(internal("boom")).0, ErrorCode::Internal);
        assert_eq!(err_parts(invalid_path("nope")).0, ErrorCode::InvalidPath);
        let (code, msg) = err_parts(invalid_name("bad name"));
        assert_eq!(code, ErrorCode::InvalidPath);
        assert_eq!(msg, "bad name");
    }
}
