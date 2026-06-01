//! Database administration ("Manage DBs"): list / create / drop databases in a
//! running SQL service.
//!
//! This is the I/O edge — it shells out to the **bundled client** for each
//! engine (`mysql`/`mariadb` over the Unix socket; `psql` over TCP loopback) and
//! captures its output. All the decision logic — name validation, SQL and `argv`
//! construction, output parsing — is pure and unit-tested in
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

/// `list databases <svc>` — the user databases (system schemas filtered out).
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

/// `drop database <svc> <name>` — refuses system databases.
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

/// `backup database <svc> <name> <path>` — stream a plain-SQL dump to `path`.
///
/// The dump tool writes to stdout; we stream that to a temp sibling of `path` and
/// atomically rename on success, so a failed dump never truncates an existing target.
/// The destination path is never passed to the dump tool — there is no path-injection
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

    // Stream to a temp sibling, rename on success (atomic; never truncates `path`).
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

    // Take both pipes and drain them concurrently with `wait()`; draining only one
    // while the other's buffer fills would deadlock the child.
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

/// `restore database <svc> <name> <path>` — replay a plain-SQL file into `name`.
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
    // `stdin` is MOVED into this future so it drops (→ EOF) when the copy finishes,
    // letting the client exit and `wait()` resolve. Keeping it alive would hang.
    let feed = async move {
        let res = tokio::io::copy(&mut file, &mut stdin).await;
        // Close the write side → EOF, so the client can finish and `wait()` resolve.
        // `stdin` is dropped at the end of this `async move` block regardless.
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

    // Database ops need a live server.
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
                "start {} first — managing databases needs a running server",
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
    if d.contains("already exists") {
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
