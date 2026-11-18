//! The shared set-Xdebug operation behind `Request::SetXdebug` (CLI, GUI, MCP).
//!
//! The settings model lives in [`yerd_core::php_xdebug`]; this module only
//! orchestrates: resolve the version, merge the request, stage and load-probe
//! the bundled `xdebug.so` when it is being switched on, then, under
//! `php_settings_mutate`, publish the staged file, persist, push the new load
//! map into the `PhpManager` and restart that one pool if its command line
//! changed.
//!
//! The download and probe run under [`FETCH_DEADLINE`] with no lock held and
//! never touch the final `.so`: a new build is written to a temp sibling,
//! probed from there and only renamed into place once the commit is certain.
//! On any failure or timeout the temp file is removed and nothing is saved.
//! The config lock is dropped before the manager lock is taken.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use yerd_core::php_xdebug::{self, XdebugMode, XdebugSettings, XdebugStart};
use yerd_core::PhpVersion;
use yerd_ipc::{ErrorCode, Response};
use yerd_php::{CommandRunner, Downloader, XdebugLoad};
use yerd_platform::PlatformDirs;

use crate::ext_install::{StagedSo, XdebugStage};
use crate::state::DaemonState;

/// Total budget for downloading and load-probing Xdebug before anything is
/// committed. Below the 120s client deadlines of MCP and the GUI, so a slow
/// download comes back as a typed error rather than a client timeout.
pub const FETCH_DEADLINE: Duration = Duration::from_secs(90);

/// The module name Xdebug registers under `[Zend Modules]` in `php -m`.
const XDEBUG_MODULE: &str = "Xdebug";

/// One `SetXdebug` request's fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XdebugRequest {
    /// The PHP version; `None` means the configured default.
    pub version: Option<PhpVersion>,
    /// Whether the version's pool should load Xdebug, or `None` to keep the
    /// current state.
    pub enabled: Option<bool>,
    /// New modes, or `None` to keep the current ones.
    pub modes: Option<Vec<XdebugMode>>,
    /// New start trigger, or `None` to keep the current one.
    pub start: Option<XdebugStart>,
}

/// What a successful [`set_xdebug_with`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applied {
    /// Nothing changed: no save, no fetch, no restart.
    ///
    /// Also the answer to a retry after a failed restart: a pool whose restart
    /// failed is no longer supervised, so the next request to the version
    /// starts it with the current load map and there is nothing left to do.
    Unchanged,
    /// The change was applied; `restart` says whether the pool's load changed
    /// (and so the pool was restarted if it was running).
    Changed {
        /// Whether the restart path was taken.
        restart: bool,
    },
}

/// Handle `Request::SetXdebug`: the production wiring around
/// [`set_xdebug_with`]. Replies with `PhpVersions` on success.
pub async fn set_xdebug(state: &DaemonState, req: XdebugRequest) -> Response {
    let dl = crate::network::downloader(state).await;
    let host = crate::ext_install::host_os_arch();
    let runner = yerd_php::TokioCommandRunner;
    match set_xdebug_with(state, &dl, &runner, host, FETCH_DEADLINE, req).await {
        Ok(_) => crate::ipc_server::php_versions_response(state).await,
        Err(resp) => resp,
    }
}

/// Injectable core of [`set_xdebug`]: the downloader, probe runner, host
/// `(os, arch)` and fetch deadline are parameters so tests run offline on any
/// CI host.
///
/// # Errors
/// The error `Response` to send back (typed `ErrorCode`, human message).
pub async fn set_xdebug_with(
    state: &DaemonState,
    dl: &dyn Downloader,
    runner: &dyn CommandRunner,
    (os, arch): (&str, &str),
    deadline: Duration,
    req: XdebugRequest,
) -> Result<Applied, Response> {
    let (v, current, registered) = {
        let cfg = state.config.lock().await;
        let v = req.version.unwrap_or(cfg.php.default);
        (
            v,
            cfg.php.xdebug.get(&v).cloned().unwrap_or_default(),
            registered_xdebug(&cfg, v),
        )
    };
    if let Some(resp) = crate::ipc_server::require_installed(v, state) {
        return Err(resp);
    }
    let new = merge(&current, &req)?;
    if v.is_legacy() {
        if new.enabled {
            return Err(error(
                ErrorCode::LegacyRestricted,
                format!("Xdebug is not available for legacy PHP {v}"),
            ));
        }
        return Ok(Applied::Unchanged);
    }
    if new.enabled && !php_xdebug::is_available(v, os, arch) {
        return Err(error(
            ErrorCode::XdebugUnavailable,
            format!("Xdebug is not built for PHP {v} on {os}/{arch}"),
        ));
    }

    let so = crate::ext_install::xdebug_so_path(&state.dirs, v);
    let unchanged = new == current && (!new.enabled || so.is_file());

    let fresh = if !unchanged && new.enabled && (!current.enabled || !so.is_file()) {
        if let Some(name) = registered {
            return Err(conflict(v, &name));
        }
        stage_bounded(state, dl, runner, (os, arch), v, &so, deadline).await?
    } else {
        None
    };

    let _mutate_guard = state.php_settings_mutate.lock().await;
    prepare_output_dir(state, v, &req).await?;
    if unchanged {
        return Ok(Applied::Unchanged);
    }
    let committed = persist(state, v, &req, fresh).await?;
    let restart = apply_xdebug(
        state,
        v,
        committed.map,
        committed.old_load.as_ref(),
        committed.new_load.as_ref(),
        committed.published,
    )
    .await?;
    tracing::info!(version = %v, restart, "applied Xdebug settings");
    Ok(Applied::Changed { restart })
}

/// [`stage_and_probe`] under `deadline`. On expiry the in-flight download or
/// probe is dropped (the probe child is killed and any staged file removed)
/// and nothing has been committed.
async fn stage_bounded(
    state: &DaemonState,
    dl: &dyn Downloader,
    runner: &dyn CommandRunner,
    host: (&str, &str),
    v: PhpVersion,
    so: &Path,
    deadline: Duration,
) -> Result<Option<StagedSo>, Response> {
    tokio::time::timeout(deadline, stage_and_probe(state, dl, runner, host, v, so))
        .await
        .unwrap_or_else(|_| {
            Err(error(
                ErrorCode::XdebugUnavailable,
                format!(
                    "timed out downloading or probing Xdebug for PHP {v} after {}s; nothing changed",
                    deadline.as_secs()
                ),
            ))
        })
}

/// Stage a new `.so` when the manifest has newer bytes, then positively
/// load-probe whichever file would be loaded (the staged one, else the one on
/// disk). Returns the staged file to publish, or `None` when the existing file
/// is current. When the manifest or download fails but a file is already on
/// disk, that file is probed so an offline enable still works. Holds no lock.
async fn stage_and_probe(
    state: &DaemonState,
    dl: &dyn Downloader,
    runner: &dyn CommandRunner,
    (os, arch): (&str, &str),
    v: PhpVersion,
    so: &Path,
) -> Result<Option<StagedSo>, Response> {
    let fresh = match crate::ext_install::stage_xdebug(&state.dirs, dl, v, os, arch).await {
        Ok(XdebugStage::Staged(staged)) => Some(staged),
        Ok(XdebugStage::Current) => None,
        Err(e) if so.is_file() => {
            tracing::warn!(version = %v, error = %e, "couldn't refresh Xdebug, using the existing file");
            None
        }
        Err(e) => {
            return Err(error(
                ErrorCode::XdebugUnavailable,
                format!("couldn't download Xdebug for PHP {v}: {e}"),
            ))
        }
    };
    let candidate = fresh.as_ref().map_or(so, StagedSo::path);
    probe(state, runner, v, candidate).await?;
    Ok(fresh)
}

async fn probe(
    state: &DaemonState,
    runner: &dyn CommandRunner,
    v: PhpVersion,
    candidate: &Path,
) -> Result<(), Response> {
    let php_bin = crate::php_install::cli_binary_path(&state.dirs, v);
    yerd_php::probe_zend_module(runner, &php_bin, candidate, XDEBUG_MODULE)
        .await
        .map_err(|e| {
            error(
                ErrorCode::ExtensionLoadFailed,
                format!("Xdebug failed to load into PHP {v}: {e}"),
            )
        })
}

/// Startup refresh: stage and positively probe a newer bundled `xdebug.so` for
/// every installed version with Xdebug enabled, all within `deadline`, then
/// publish each one and restart its running pool. Without the restart, a pool
/// lazily started before the refresh finished would keep running with a stale
/// Xdebug. Returns the replaced versions.
///
/// A build that fails its probe is discarded and the last good file kept. Each
/// publish runs under `php_settings_mutate` and only if the daemon isn't
/// shutting down and the version is still installed with managed Xdebug on.
/// Shutdown is re-checked once the locks are held, since it may have started
/// while waiting for them.
pub async fn refresh_enabled(
    state: &DaemonState,
    dl: &dyn Downloader,
    runner: &dyn CommandRunner,
    (os, arch): (&str, &str),
    deadline: Duration,
) -> Vec<PhpVersion> {
    let installed = crate::ext_install::installed_versions(&state.dirs);
    let versions = enabled_versions(&*state.config.lock().await, &installed);
    let Ok(staged) = tokio::time::timeout(
        deadline,
        stage_and_probe_all(state, dl, runner, (os, arch), &versions),
    )
    .await
    else {
        tracing::warn!(
            seconds = deadline.as_secs(),
            "timed out refreshing Xdebug; keeping the existing files"
        );
        return Vec::new();
    };
    let mut replaced = Vec::new();
    for (v, so) in staged {
        if shutting_down(state) {
            break;
        }
        let _mutate_guard = state.php_settings_mutate.lock().await;
        let map = {
            let cfg = state.config.lock().await;
            if shutting_down(state) {
                break;
            }
            let map = xdebug_load_map(&cfg, &state.dirs);
            if !map.contains_key(&v)
                || !crate::php_install::cli_binary_path(&state.dirs, v).exists()
            {
                continue;
            }
            if let Err(e) = so.publish() {
                tracing::warn!(version = %v, error = %e, "failed to publish the refreshed Xdebug");
                continue;
            }
            map
        };
        let Some(load) = map.get(&v).cloned() else {
            continue;
        };
        if let Err(resp) = apply_xdebug(state, v, map, Some(&load), Some(&load), true).await {
            tracing::warn!(version = %v, response = ?resp, "refreshed Xdebug but the pool restart failed");
        }
        tracing::info!(version = %v, "refreshed Xdebug and restarted its pool if running");
        replaced.push(v);
    }
    replaced
}

async fn stage_and_probe_all(
    state: &DaemonState,
    dl: &dyn Downloader,
    runner: &dyn CommandRunner,
    (os, arch): (&str, &str),
    versions: &[PhpVersion],
) -> Vec<(PhpVersion, StagedSo)> {
    let mut ok = Vec::new();
    for (v, so) in crate::ext_install::stage_xdebug_for(&state.dirs, dl, versions, os, arch).await {
        match probe(state, runner, v, so.path()).await {
            Ok(()) => ok.push((v, so)),
            Err(resp) => {
                tracing::warn!(version = %v, response = ?resp, "discarding a refreshed Xdebug that failed its probe");
            }
        }
    }
    ok
}

fn shutting_down(state: &DaemonState) -> bool {
    *state.shutdown_tx.borrow()
}

/// Create the output dir when the request's merged result for `v` is enabled
/// and writes output, so a failure returns before anything is saved. An
/// Unchanged request passes through here too, which repairs a dir removed
/// since the last save. The caller holds `php_settings_mutate`, which every
/// writer of `[php.xdebug]` takes, so the merge here matches the one
/// [`persist`] saves; the config lock is released before touching the
/// filesystem.
///
/// # Errors
/// The merge's validation error, or the actionable output-dir error.
async fn prepare_output_dir(
    state: &DaemonState,
    v: PhpVersion,
    req: &XdebugRequest,
) -> Result<(), Response> {
    let current = state
        .config
        .lock()
        .await
        .php
        .xdebug
        .get(&v)
        .cloned()
        .unwrap_or_default();
    let merged = merge(&current, req)?;
    if merged.enabled && merged.writes_output() {
        create_output_dir(&state.dirs)?;
    }
    Ok(())
}

/// What [`persist`] committed.
struct Committed {
    map: BTreeMap<PhpVersion, XdebugLoad>,
    old_load: Option<XdebugLoad>,
    new_load: Option<XdebugLoad>,
    published: bool,
}

/// Re-check and persist the request against the fresh config under the config
/// lock (the caller holds `php_settings_mutate`), publishing `fresh` first when
/// the merged result is enabled. An enabled result with nothing staged needs
/// the existing `xdebug.so` to still be on disk (an uninstall may have removed
/// it since the probe). The output dir was already prepared by
/// [`prepare_output_dir`]. A version whose settings end up equal to the default
/// has its entry removed rather than stored. Dropping `fresh` on any other
/// path removes its temp file.
async fn persist(
    state: &DaemonState,
    v: PhpVersion,
    req: &XdebugRequest,
    fresh: Option<StagedSo>,
) -> Result<Committed, Response> {
    let mut cfg_guard = state.config.lock().await;
    if shutting_down(state) {
        return Err(crate::ipc_server::internal(
            "the daemon is shutting down; nothing changed".to_owned(),
        ));
    }
    let old_load = xdebug_load_map(&cfg_guard, &state.dirs).remove(&v);
    if !crate::php_install::cli_binary_path(&state.dirs, v).exists() {
        return Err(error(
            ErrorCode::NotFound,
            format!("PHP {v} is not installed - run `yerd install php {v}`"),
        ));
    }
    let current = cfg_guard.php.xdebug.get(&v).cloned().unwrap_or_default();
    let merged = merge(&current, req)?;
    if merged.enabled {
        if let Some(name) = registered_xdebug(&cfg_guard, v) {
            return Err(conflict(v, &name));
        }
    }
    let published = match (merged.enabled, fresh) {
        (true, Some(staged)) => {
            staged.publish().map_err(|e| {
                crate::ipc_server::internal(format!("couldn't install Xdebug for PHP {v}: {e}"))
            })?;
            true
        }
        (true, None) if !crate::ext_install::xdebug_so_path(&state.dirs, v).is_file() => {
            return Err(error(
                ErrorCode::XdebugUnavailable,
                format!(
                    "the Xdebug library for PHP {v} is missing; nothing changed, retry to \
                     download it again"
                ),
            ));
        }
        _ => false,
    };
    let mut next = cfg_guard.clone();
    if merged == XdebugSettings::default() {
        next.php.xdebug.remove(&v);
    } else {
        next.php.xdebug.insert(v, merged);
    }
    if next.php.xdebug != cfg_guard.php.xdebug {
        if let Err(e) = next.validate() {
            return Err(crate::ipc_server::internal(format!(
                "config validation failed: {e}"
            )));
        }
        if let Err(e) = next.save(&state.config_path) {
            return Err(crate::ipc_server::internal(format!(
                "config save failed: {e}"
            )));
        }
        *cfg_guard = next;
    }
    let map = xdebug_load_map(&cfg_guard, &state.dirs);
    let new_load = map.get(&v).cloned();
    Ok(Committed {
        map,
        old_load,
        new_load,
        published,
    })
}

/// Push `new_map` into the manager and restart `v`'s pool when
/// [`should_restart`] says so and the pool has been started. Returns the
/// restart decision.
///
/// # Errors
/// An `Internal` response when the restart fails. The settings are already
/// saved; the failed pool is no longer supervised, so the next request (or a
/// manual restart) starts it with the new load.
async fn apply_xdebug(
    state: &DaemonState,
    v: PhpVersion,
    new_map: BTreeMap<PhpVersion, XdebugLoad>,
    old_load: Option<&XdebugLoad>,
    new_load: Option<&XdebugLoad>,
    fetched_this_call: bool,
) -> Result<bool, Response> {
    let restart = should_restart(old_load, new_load, fetched_this_call);
    let mut mgr = state.php_manager.lock().await;
    mgr.set_xdebug(new_map);
    if restart && mgr.snapshots().iter().any(|s| s.version == v) {
        if let Err(e) = mgr.restart(v).await {
            tracing::warn!(version = %v, error = %e, "failed to restart FPM pool after Xdebug change");
            return Err(crate::ipc_server::internal(restart_failed_message(v, &e)));
        }
    }
    Ok(restart)
}

/// The message for a pool that failed to restart after its Xdebug settings
/// were saved.
fn restart_failed_message(v: PhpVersion, e: &dyn std::fmt::Display) -> String {
    format!(
        "Xdebug settings saved, but PHP {v} failed to restart: {e}. Reloading a site that \
         uses PHP {v} starts it with the new settings, or run `yerd restart php {v}` to try \
         again"
    )
}

/// True when the pool's Xdebug load differs (on/off, path or directives).
#[must_use]
pub fn load_changed(old: Option<&XdebugLoad>, new: Option<&XdebugLoad>) -> bool {
    old != new
}

/// Restart when the load changed, or when the `.so` was just re-downloaded
/// (same path, new bytes).
#[must_use]
pub fn should_restart(
    old: Option<&XdebugLoad>,
    new: Option<&XdebugLoad>,
    fetched_this_call: bool,
) -> bool {
    load_changed(old, new) || fetched_this_call
}

/// The `PhpManager` Xdebug map for `cfg`: every enabled, non-legacy version
/// without a hand-registered Xdebug, with its managed `-d` defines.
#[must_use]
pub fn xdebug_load_map(
    cfg: &yerd_config::Config,
    dirs: &PlatformDirs,
) -> BTreeMap<PhpVersion, XdebugLoad> {
    let out_dir = output_dir(dirs).to_string_lossy().into_owned();
    cfg.php
        .xdebug
        .iter()
        .filter(|(v, s)| s.enabled && !v.is_legacy() && registered_xdebug(cfg, **v).is_none())
        .map(|(v, s)| {
            let defines = php_xdebug::managed_directives(s, &out_dir)
                .into_iter()
                .map(|(k, val)| (k.to_owned(), val))
                .collect();
            (
                *v,
                XdebugLoad {
                    path: crate::ext_install::xdebug_so_path(dirs, *v),
                    defines,
                },
            )
        })
        .collect()
}

/// Installed, non-legacy versions with Xdebug switched on in `cfg` (the
/// startup refresh set).
#[must_use]
pub fn enabled_versions(cfg: &yerd_config::Config, installed: &[PhpVersion]) -> Vec<PhpVersion> {
    cfg.php
        .xdebug
        .iter()
        .filter(|(v, s)| s.enabled && !v.is_legacy() && installed.contains(v))
        .map(|(v, _)| *v)
        .collect()
}

/// The name of a hand-registered Xdebug extension for `v`, if any.
pub(crate) fn registered_xdebug(cfg: &yerd_config::Config, v: PhpVersion) -> Option<String> {
    cfg.php
        .extensions
        .get(&v)?
        .iter()
        .find(|e| php_xdebug::is_xdebug_extension(&e.name, &e.path))
        .map(|e| e.name.clone())
}

/// Startup: create the output dir when any enabled version writes files
/// there, so a pool started before the first `SetXdebug` can write them. A
/// failure is logged; the next `SetXdebug` for such a version reports it.
pub fn ensure_output_dir(cfg: &yerd_config::Config, dirs: &PlatformDirs) {
    if needs_output_dir(cfg) {
        if let Err(resp) = create_output_dir(dirs) {
            tracing::warn!(response = ?resp, "failed to create the Xdebug output dir");
        }
    }
}

/// True when some version has Xdebug on with a mode that writes to
/// `xdebug.output_dir`.
#[must_use]
pub fn needs_output_dir(cfg: &yerd_config::Config) -> bool {
    cfg.php
        .xdebug
        .iter()
        .any(|(v, s)| s.enabled && !v.is_legacy() && s.writes_output())
}

/// Create `{data}/xdebug`, or the actionable error to return before anything
/// that writes output there is saved.
fn create_output_dir(dirs: &PlatformDirs) -> Result<(), Response> {
    let dir = output_dir(dirs);
    std::fs::create_dir_all(&dir).map_err(|e| output_dir_error(&dir, &e))
}

fn output_dir_error(dir: &Path, e: &dyn std::fmt::Display) -> Response {
    crate::ipc_server::internal(format!(
        "couldn't create the Xdebug output directory {}: {e}. Profile, trace and gcstats \
         files are written there; make sure it can be created as a directory (for example \
         remove a file at that path), then try again. Nothing changed",
        dir.display()
    ))
}

/// `{data}/xdebug`, where profile, trace and gcstats files are written.
pub(crate) fn output_dir(dirs: &PlatformDirs) -> std::path::PathBuf {
    dirs.data.join("xdebug")
}

/// `{data}/php-ext/php-<v>/debug.ini`, written by the `phpdebug` shims.
#[must_use]
pub fn debug_ini_path(dirs: &PlatformDirs, v: PhpVersion) -> std::path::PathBuf {
    crate::ext_install::xdebug_so_path(dirs, v)
        .parent()
        .map_or_else(
            || dirs.data.join("debug.ini"),
            |p: &Path| p.join("debug.ini"),
        )
}

fn merge(current: &XdebugSettings, req: &XdebugRequest) -> Result<XdebugSettings, Response> {
    php_xdebug::merge_request(Some(current), req.enabled, req.modes.clone(), req.start)
        .map_err(|e| error(ErrorCode::InvalidSetting, e.to_string()))
}

fn conflict(v: PhpVersion, name: &str) -> Response {
    error(
        ErrorCode::AlreadyExists,
        format!(
            "a hand-registered Xdebug (`{name}`) is loaded for PHP {v}; remove it first with \
             `yerd php ext remove {v} {name}`"
        ),
    )
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
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::test_support::state_in;

    const SO_BYTES: &[u8] = b"xdebug-so";

    struct FakeDownloader {
        routes: Vec<(String, Vec<u8>)>,
        calls: AtomicUsize,
    }

    impl FakeDownloader {
        fn serving_84() -> Self {
            let manifest = serde_json::json!({"files": [{
                "name": "xdebug-8.4-macos-aarch64.so",
                "php": "8.4",
                "os": "macos",
                "arch": "aarch64",
                "sha256": crate::ext_install::sha256_hex(SO_BYTES),
            }]})
            .to_string()
            .into_bytes();
            Self {
                routes: vec![
                    ("/xdebug-manifest.json".to_owned(), manifest),
                    ("/xdebug-8.4-macos-aarch64.so".to_owned(), SO_BYTES.to_vec()),
                ],
                calls: AtomicUsize::new(0),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::Relaxed)
        }
    }

    #[async_trait::async_trait]
    impl Downloader for FakeDownloader {
        async fn download(&self, url: &str) -> Result<Vec<u8>, yerd_php::DownloadError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            self.routes
                .iter()
                .find(|(suffix, _)| url.ends_with(suffix.as_str()))
                .map(|(_, body)| body.clone())
                .ok_or_else(|| yerd_php::DownloadError::Transport {
                    url: url.to_owned(),
                    reason: "no route".into(),
                })
        }
    }

    struct FakeRunner {
        ok: bool,
        listed: bool,
        calls: AtomicUsize,
    }

    impl FakeRunner {
        fn new(ok: bool) -> Self {
            Self {
                ok,
                listed: true,
                calls: AtomicUsize::new(0),
            }
        }

        fn unlisted() -> Self {
            Self {
                listed: false,
                ..Self::new(true)
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::Relaxed)
        }
    }

    #[async_trait::async_trait]
    impl CommandRunner for FakeRunner {
        async fn run(&self, _cmd: std::process::Command) -> std::io::Result<yerd_php::ProbeOutput> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            let stderr = if self.ok {
                String::new()
            } else {
                "Warning: Failed loading Zend extension 'xdebug.so'".to_owned()
            };
            let stdout = if self.listed {
                "[PHP Modules]\nCore\n\n[Zend Modules]\nXdebug\n"
            } else {
                "[PHP Modules]\nCore\n\n[Zend Modules]\n"
            };
            Ok(yerd_php::ProbeOutput {
                status_ok: self.ok,
                stdout: stdout.to_owned(),
                stderr,
            })
        }
    }

    fn fake_install(dirs: &PlatformDirs, v: PhpVersion) {
        let base = dirs
            .data
            .join("php")
            .join(format!("php-{}.{}", v.major, v.minor));
        std::fs::create_dir_all(base.join("sbin")).unwrap();
        std::fs::create_dir_all(base.join("bin")).unwrap();
        std::fs::write(base.join("sbin").join("php-fpm"), b"x").unwrap();
        std::fs::write(base.join("bin").join("php"), b"x").unwrap();
    }

    fn req(version: Option<PhpVersion>, enabled: bool) -> XdebugRequest {
        XdebugRequest {
            version,
            enabled: Some(enabled),
            modes: None,
            start: None,
        }
    }

    const V84: PhpVersion = PhpVersion::new(8, 4);

    async fn run(
        state: &DaemonState,
        dl: &FakeDownloader,
        runner: &FakeRunner,
        r: XdebugRequest,
    ) -> Result<Applied, Response> {
        set_xdebug_with(state, dl, runner, HOST, FETCH_DEADLINE, r).await
    }

    const HOST: (&str, &str) = ("macos", "aarch64");

    fn error_code(resp: Response) -> ErrorCode {
        match resp {
            Response::Error { code, .. } => code,
            other => panic!("expected an error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn enable_downloads_probes_and_persists_then_repeat_is_a_no_op() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install(&state.dirs, V84);
        let dl = FakeDownloader::serving_84();
        let runner = FakeRunner::new(true);

        let got = run(&state, &dl, &runner, req(Some(V84), true))
            .await
            .unwrap();
        assert_eq!(got, Applied::Changed { restart: true });
        assert_eq!(
            std::fs::read(crate::ext_install::xdebug_so_path(&state.dirs, V84)).unwrap(),
            SO_BYTES
        );
        assert_eq!(runner.calls(), 1);
        assert!(
            state
                .config
                .lock()
                .await
                .php
                .xdebug
                .get(&V84)
                .unwrap()
                .enabled
        );
        let saved = yerd_config::Config::load(&state.config_path).unwrap();
        assert!(saved.php.xdebug.get(&V84).unwrap().enabled);

        std::fs::remove_file(&state.config_path).unwrap();
        let downloads = dl.calls();
        let again = run(&state, &dl, &runner, req(Some(V84), true))
            .await
            .unwrap();
        assert_eq!(again, Applied::Unchanged);
        assert_eq!(dl.calls(), downloads);
        assert_eq!(runner.calls(), 1);
        assert!(!state.config_path.exists());
    }

    #[tokio::test]
    async fn off_on_unconfigured_version_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install(&state.dirs, V84);
        let dl = FakeDownloader::serving_84();
        let runner = FakeRunner::new(true);
        let got = run(&state, &dl, &runner, req(Some(V84), false))
            .await
            .unwrap();
        assert_eq!(got, Applied::Unchanged);
        assert!(state.config.lock().await.php.xdebug.is_empty());
        assert_eq!(dl.calls(), 0);
        assert!(!state.config_path.exists());
        assert!(!load_changed(None, None));
    }

    #[tokio::test]
    async fn modes_change_while_disabled_persists_without_restart() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install(&state.dirs, V84);
        let dl = FakeDownloader::serving_84();
        let runner = FakeRunner::new(true);
        let r = XdebugRequest {
            version: Some(V84),
            enabled: Some(false),
            modes: Some(vec![XdebugMode::Profile]),
            start: Some(XdebugStart::Trigger),
        };
        let got = run(&state, &dl, &runner, r).await.unwrap();
        assert_eq!(got, Applied::Changed { restart: false });
        let cfg = state.config.lock().await;
        let entry = cfg.php.xdebug.get(&V84).unwrap();
        assert!(!entry.enabled);
        assert_eq!(entry.modes, vec![XdebugMode::Profile]);
        assert_eq!(dl.calls(), 0);
        assert_eq!(runner.calls(), 0);
    }

    #[tokio::test]
    async fn off_after_on_with_defaults_removes_the_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install(&state.dirs, V84);
        let dl = FakeDownloader::serving_84();
        let runner = FakeRunner::new(true);
        run(&state, &dl, &runner, req(Some(V84), true))
            .await
            .unwrap();
        let got = run(&state, &dl, &runner, req(Some(V84), false))
            .await
            .unwrap();
        assert_eq!(got, Applied::Changed { restart: true });
        assert!(state.config.lock().await.php.xdebug.is_empty());
        let saved = yerd_config::Config::load(&state.config_path).unwrap();
        assert!(saved.php.xdebug.is_empty());
    }

    #[tokio::test]
    async fn legacy_enable_is_refused_and_disable_is_a_no_op() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let v74 = PhpVersion::new(7, 4);
        fake_install(&state.dirs, v74);
        let dl = FakeDownloader::serving_84();
        let runner = FakeRunner::new(true);
        let err = run(&state, &dl, &runner, req(Some(v74), true))
            .await
            .unwrap_err();
        assert_eq!(error_code(err), ErrorCode::LegacyRestricted);
        let off = run(&state, &dl, &runner, req(Some(v74), false))
            .await
            .unwrap();
        assert_eq!(off, Applied::Unchanged);
        assert!(state.config.lock().await.php.xdebug.is_empty());
        assert!(!state.config_path.exists());
    }

    #[tokio::test]
    async fn unsupported_triple_is_unavailable() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install(&state.dirs, V84);
        let dl = FakeDownloader::serving_84();
        let runner = FakeRunner::new(true);
        let err = set_xdebug_with(
            &state,
            &dl,
            &runner,
            ("macos", "x86_64"),
            FETCH_DEADLINE,
            req(Some(V84), true),
        )
        .await
        .unwrap_err();
        assert_eq!(error_code(err), ErrorCode::XdebugUnavailable);
        assert_eq!(dl.calls(), 0);
    }

    #[tokio::test]
    async fn download_failure_is_unavailable() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install(&state.dirs, V84);
        let dl = FakeDownloader {
            routes: Vec::new(),
            calls: AtomicUsize::new(0),
        };
        let runner = FakeRunner::new(true);
        let err = run(&state, &dl, &runner, req(Some(V84), true))
            .await
            .unwrap_err();
        assert_eq!(error_code(err), ErrorCode::XdebugUnavailable);
        assert!(state.config.lock().await.php.xdebug.is_empty());
    }

    #[tokio::test]
    async fn hand_registered_xdebug_conflicts() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install(&state.dirs, V84);
        state.config.lock().await.php.extensions.insert(
            V84,
            vec![yerd_config::ExtEntry {
                name: "debugger".into(),
                path: "/opt/ext/xdebug.so".into(),
                zend: true,
            }],
        );
        let dl = FakeDownloader::serving_84();
        let runner = FakeRunner::new(true);
        let err = run(&state, &dl, &runner, req(Some(V84), true))
            .await
            .unwrap_err();
        assert_eq!(error_code(err), ErrorCode::AlreadyExists);
        assert_eq!(dl.calls(), 0);
        assert!(state.config.lock().await.php.xdebug.is_empty());
    }

    #[tokio::test]
    async fn probe_failure_leaves_config_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install(&state.dirs, V84);
        let dl = FakeDownloader::serving_84();
        let runner = FakeRunner::new(false);
        let err = run(&state, &dl, &runner, req(Some(V84), true))
            .await
            .unwrap_err();
        assert_eq!(error_code(err), ErrorCode::ExtensionLoadFailed);
        assert_eq!(runner.calls(), 1);
        assert!(state.config.lock().await.php.xdebug.is_empty());
        assert!(!state.config_path.exists());
    }

    #[tokio::test]
    async fn no_version_resolves_the_default() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install(&state.dirs, V84);
        state.config.lock().await.php.default = V84;
        let dl = FakeDownloader::serving_84();
        let runner = FakeRunner::new(true);
        run(&state, &dl, &runner, req(None, true)).await.unwrap();
        assert!(
            state
                .config
                .lock()
                .await
                .php
                .xdebug
                .get(&V84)
                .unwrap()
                .enabled
        );
    }

    #[tokio::test]
    async fn empty_modes_is_invalid_and_missing_install_is_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let dl = FakeDownloader::serving_84();
        let runner = FakeRunner::new(true);
        let missing = run(&state, &dl, &runner, req(Some(V84), true))
            .await
            .unwrap_err();
        assert_eq!(error_code(missing), ErrorCode::NotFound);
        fake_install(&state.dirs, V84);
        let r = XdebugRequest {
            version: Some(V84),
            enabled: Some(true),
            modes: Some(Vec::new()),
            start: None,
        };
        let err = run(&state, &dl, &runner, r).await.unwrap_err();
        assert_eq!(error_code(err), ErrorCode::InvalidSetting);
        let gc = XdebugRequest {
            version: Some(V84),
            enabled: Some(true),
            modes: Some(vec![XdebugMode::Gcstats]),
            start: Some(XdebugStart::Trigger),
        };
        let err = run(&state, &dl, &runner, gc).await.unwrap_err();
        assert_eq!(error_code(err), ErrorCode::InvalidSetting);
        assert_eq!(dl.calls(), 0);
    }

    #[tokio::test]
    async fn enabled_entry_with_missing_so_refetches_and_restarts() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install(&state.dirs, V84);
        let dl = FakeDownloader::serving_84();
        let runner = FakeRunner::new(true);
        run(&state, &dl, &runner, req(Some(V84), true))
            .await
            .unwrap();
        std::fs::remove_file(crate::ext_install::xdebug_so_path(&state.dirs, V84)).unwrap();
        let downloads = dl.calls();
        let got = run(&state, &dl, &runner, req(Some(V84), true))
            .await
            .unwrap();
        assert_eq!(got, Applied::Changed { restart: true });
        assert_eq!(dl.calls(), downloads + 2);
        assert_eq!(runner.calls(), 2);
        assert!(crate::ext_install::xdebug_so_path(&state.dirs, V84).is_file());
    }

    fn write_so(state: &DaemonState, bytes: &[u8]) -> std::path::PathBuf {
        let so = crate::ext_install::xdebug_so_path(&state.dirs, V84);
        std::fs::create_dir_all(so.parent().unwrap()).unwrap();
        std::fs::write(&so, bytes).unwrap();
        so
    }

    #[tokio::test]
    async fn enable_replaces_a_stale_so() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install(&state.dirs, V84);
        let so = write_so(&state, b"stale");
        let dl = FakeDownloader::serving_84();
        let runner = FakeRunner::new(true);
        let got = run(&state, &dl, &runner, req(Some(V84), true))
            .await
            .unwrap();
        assert_eq!(got, Applied::Changed { restart: true });
        assert_eq!(std::fs::read(&so).unwrap(), SO_BYTES);
        assert_eq!(dl.calls(), 2);
    }

    #[tokio::test]
    async fn enable_with_current_so_fetches_only_the_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install(&state.dirs, V84);
        write_so(&state, SO_BYTES);
        let dl = FakeDownloader::serving_84();
        let runner = FakeRunner::new(true);
        let got = run(&state, &dl, &runner, req(Some(V84), true))
            .await
            .unwrap();
        assert_eq!(got, Applied::Changed { restart: true });
        assert_eq!(dl.calls(), 1);
        assert_eq!(runner.calls(), 1);
    }

    #[tokio::test]
    async fn offline_enable_uses_the_existing_so() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install(&state.dirs, V84);
        let so = write_so(&state, b"older-build");
        let dl = FakeDownloader {
            routes: Vec::new(),
            calls: AtomicUsize::new(0),
        };
        let runner = FakeRunner::new(true);
        let got = run(&state, &dl, &runner, req(Some(V84), true))
            .await
            .unwrap();
        assert_eq!(got, Applied::Changed { restart: true });
        assert_eq!(std::fs::read(&so).unwrap(), b"older-build");
        assert_eq!(runner.calls(), 1);
        assert!(
            state
                .config
                .lock()
                .await
                .php
                .xdebug
                .get(&V84)
                .unwrap()
                .enabled
        );
    }

    fn enable_in_config(cfg: &mut yerd_config::Config, v: PhpVersion) {
        cfg.php.xdebug.insert(
            v,
            XdebugSettings {
                enabled: true,
                ..XdebugSettings::default()
            },
        );
    }

    async fn refresh(
        state: &DaemonState,
        dl: &dyn Downloader,
        runner: &FakeRunner,
    ) -> Vec<PhpVersion> {
        refresh_enabled(state, dl, runner, HOST, FETCH_DEADLINE).await
    }

    fn temp_files(state: &DaemonState) -> Vec<std::path::PathBuf> {
        let dir = crate::ext_install::xdebug_so_path(&state.dirs, V84)
            .parent()
            .unwrap()
            .to_path_buf();
        std::fs::read_dir(dir)
            .map(|rd| {
                rd.map(|e| e.unwrap().path())
                    .filter(|p| p.to_string_lossy().ends_with(".tmp"))
                    .collect()
            })
            .unwrap_or_default()
    }

    #[tokio::test]
    async fn startup_refresh_replaces_stale_installed_versions_only() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install(&state.dirs, V84);
        let so = write_so(&state, b"stale");
        {
            let mut cfg = state.config.lock().await;
            enable_in_config(&mut cfg, V84);
            enable_in_config(&mut cfg, PhpVersion::new(8, 3));
        }
        let dl = FakeDownloader::serving_84();
        let replaced = refresh(&state, &dl, &FakeRunner::new(true)).await;
        assert_eq!(replaced, vec![V84]);
        assert_eq!(std::fs::read(&so).unwrap(), SO_BYTES);
        assert_eq!(dl.calls(), 2);
        assert!(!crate::ext_install::xdebug_so_path(&state.dirs, PhpVersion::new(8, 3)).exists());

        let again = refresh(&state, &dl, &FakeRunner::new(true)).await;
        assert!(again.is_empty());
        assert_eq!(dl.calls(), 3);
    }

    #[tokio::test]
    async fn startup_refresh_without_installed_enabled_versions_makes_no_calls() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        enable_in_config(&mut *state.config.lock().await, V84);
        let dl = FakeDownloader::serving_84();
        let replaced = refresh(&state, &dl, &FakeRunner::new(true)).await;
        assert!(replaced.is_empty());
        assert_eq!(dl.calls(), 0);
    }

    #[tokio::test]
    async fn startup_refresh_keeps_the_last_good_file_when_the_probe_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install(&state.dirs, V84);
        let so = write_so(&state, b"last-good");
        enable_in_config(&mut *state.config.lock().await, V84);
        let dl = FakeDownloader::serving_84();
        for runner in [FakeRunner::new(false), FakeRunner::unlisted()] {
            let replaced = refresh(&state, &dl, &runner).await;
            assert!(replaced.is_empty());
            assert_eq!(runner.calls(), 1);
            assert_eq!(std::fs::read(&so).unwrap(), b"last-good");
            assert!(temp_files(&state).is_empty());
        }
    }

    #[tokio::test]
    async fn startup_refresh_publishes_nothing_while_shutting_down() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install(&state.dirs, V84);
        let so = write_so(&state, b"stale");
        enable_in_config(&mut *state.config.lock().await, V84);
        state.shutdown_tx.send_replace(true);
        let dl = FakeDownloader::serving_84();
        let replaced = refresh(&state, &dl, &FakeRunner::new(true)).await;
        assert!(replaced.is_empty());
        assert_eq!(std::fs::read(&so).unwrap(), b"stale");
        assert!(temp_files(&state).is_empty());
    }

    #[tokio::test]
    async fn omitted_enabled_keeps_the_current_state() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install(&state.dirs, V84);
        let dl = FakeDownloader::serving_84();
        let runner = FakeRunner::new(true);
        run(&state, &dl, &runner, req(Some(V84), true))
            .await
            .unwrap();
        let downloads = dl.calls();
        let modes_only = XdebugRequest {
            version: Some(V84),
            enabled: None,
            modes: Some(vec![XdebugMode::Trace]),
            start: None,
        };
        let got = run(&state, &dl, &runner, modes_only).await.unwrap();
        assert_eq!(got, Applied::Changed { restart: true });
        assert_eq!(dl.calls(), downloads);
        let entry = state
            .config
            .lock()
            .await
            .php
            .xdebug
            .get(&V84)
            .cloned()
            .unwrap();
        assert!(entry.enabled);
        assert_eq!(entry.modes, vec![XdebugMode::Trace]);
        assert!(output_dir(&state.dirs).is_dir());

        let tmp2 = tempfile::tempdir().unwrap();
        let off_state = state_in(tmp2.path());
        fake_install(&off_state.dirs, V84);
        let start_only = XdebugRequest {
            version: Some(V84),
            enabled: None,
            modes: Some(vec![XdebugMode::Profile]),
            start: Some(XdebugStart::Trigger),
        };
        let got = run(&off_state, &dl, &runner, start_only).await.unwrap();
        assert_eq!(got, Applied::Changed { restart: false });
        let entry = off_state
            .config
            .lock()
            .await
            .php
            .xdebug
            .get(&V84)
            .cloned()
            .unwrap();
        assert!(!entry.enabled);
        assert!(!output_dir(&off_state.dirs).exists());
    }

    struct SlowDownloader;

    #[async_trait::async_trait]
    impl Downloader for SlowDownloader {
        async fn download(&self, _url: &str) -> Result<Vec<u8>, yerd_php::DownloadError> {
            tokio::time::sleep(Duration::from_secs(30)).await;
            Ok(Vec::new())
        }
    }

    #[tokio::test]
    async fn fetch_past_the_deadline_is_unavailable_and_changes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install(&state.dirs, V84);
        let runner = FakeRunner::new(true);
        let err = set_xdebug_with(
            &state,
            &SlowDownloader,
            &runner,
            HOST,
            Duration::from_millis(20),
            req(Some(V84), true),
        )
        .await
        .unwrap_err();
        match err {
            Response::Error { code, message } => {
                assert_eq!(code, ErrorCode::XdebugUnavailable);
                assert!(message.contains("timed out"), "{message}");
                assert!(message.contains("nothing changed"), "{message}");
            }
            other => panic!("expected an error, got {other:?}"),
        }
        assert!(state.config.lock().await.php.xdebug.is_empty());
        assert!(!state.config_path.exists());
        assert_eq!(runner.calls(), 0);
    }

    #[tokio::test]
    async fn failed_probe_of_a_new_build_keeps_the_last_good_file() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install(&state.dirs, V84);
        let so = write_so(&state, b"last-good");
        let dl = FakeDownloader::serving_84();
        for runner in [FakeRunner::new(false), FakeRunner::unlisted()] {
            let err = run(&state, &dl, &runner, req(Some(V84), true))
                .await
                .unwrap_err();
            assert_eq!(error_code(err), ErrorCode::ExtensionLoadFailed);
            assert_eq!(std::fs::read(&so).unwrap(), b"last-good");
            assert!(temp_files(&state).is_empty());
            assert!(state.config.lock().await.php.xdebug.is_empty());
        }
    }

    #[tokio::test]
    async fn probe_must_list_xdebug_as_loaded() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install(&state.dirs, V84);
        let dl = FakeDownloader::serving_84();
        let runner = FakeRunner::unlisted();
        let err = run(&state, &dl, &runner, req(Some(V84), true))
            .await
            .unwrap_err();
        assert_eq!(error_code(err), ErrorCode::ExtensionLoadFailed);
        assert!(!crate::ext_install::xdebug_so_path(&state.dirs, V84).exists());
        assert!(temp_files(&state).is_empty());
    }

    #[test]
    fn restart_failure_message_names_the_version_and_the_recovery() {
        let msg = restart_failed_message(V84, &"spawn failed");
        assert!(msg.contains("settings saved"), "{msg}");
        assert!(
            msg.contains("PHP 8.4 failed to restart: spawn failed"),
            "{msg}"
        );
        assert!(msg.contains("`yerd restart php 8.4`"), "{msg}");
        assert!(msg.contains("Reloading a site that uses PHP 8.4"), "{msg}");
    }

    #[tokio::test]
    async fn enable_commit_without_a_staged_file_requires_the_library() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install(&state.dirs, V84);
        let err = persist(&state, V84, &req(Some(V84), true), None)
            .await
            .err()
            .unwrap();
        match err {
            Response::Error { code, message } => {
                assert_eq!(code, ErrorCode::XdebugUnavailable);
                assert!(message.contains("missing"), "{message}");
                assert!(message.contains("retry"), "{message}");
            }
            other => panic!("expected an error, got {other:?}"),
        }
        assert!(state.config.lock().await.php.xdebug.is_empty());
        assert!(!state.config_path.exists());

        write_so(&state, SO_BYTES);
        let committed = persist(&state, V84, &req(Some(V84), true), None)
            .await
            .ok()
            .unwrap();
        assert!(committed.new_load.is_some());
        assert!(!committed.published);
    }

    #[tokio::test]
    async fn output_mode_with_a_blocked_output_dir_saves_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install(&state.dirs, V84);
        std::fs::create_dir_all(&state.dirs.data).unwrap();
        std::fs::write(output_dir(&state.dirs), b"not a dir").unwrap();
        let dl = FakeDownloader::serving_84();
        let runner = FakeRunner::new(true);
        let profile = XdebugRequest {
            version: Some(V84),
            enabled: Some(true),
            modes: Some(vec![XdebugMode::Profile]),
            start: None,
        };
        let err = run(&state, &dl, &runner, profile.clone())
            .await
            .unwrap_err();
        match err {
            Response::Error { code, message } => {
                assert_eq!(code, ErrorCode::Internal);
                assert!(message.contains("Xdebug output directory"), "{message}");
                assert!(message.contains("Nothing changed"), "{message}");
            }
            other => panic!("expected an error, got {other:?}"),
        }
        assert!(state.config.lock().await.php.xdebug.is_empty());
        assert!(!state.config_path.exists());
        assert!(!crate::ext_install::xdebug_so_path(&state.dirs, V84).exists());
        assert_ne!(dl.calls(), 0);
        assert!(temp_files(&state).is_empty());

        let debug_only = run(&state, &dl, &runner, req(Some(V84), true))
            .await
            .unwrap();
        assert_eq!(debug_only, Applied::Changed { restart: true });
        let disabled_profile = XdebugRequest {
            enabled: Some(false),
            ..profile.clone()
        };
        let got = run(&state, &dl, &runner, disabled_profile).await.unwrap();
        assert_eq!(got, Applied::Changed { restart: true });

        state.config.lock().await.php.xdebug.insert(
            V84,
            XdebugSettings {
                enabled: true,
                modes: vec![XdebugMode::Profile],
                start: XdebugStart::Trigger,
            },
        );
        std::fs::remove_file(output_dir(&state.dirs)).unwrap();
        let unchanged = run(&state, &dl, &runner, profile).await.unwrap();
        assert_eq!(unchanged, Applied::Unchanged);
        assert!(output_dir(&state.dirs).is_dir());
    }

    #[tokio::test]
    async fn startup_refresh_rechecks_shutdown_after_waiting_for_the_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install(&state.dirs, V84);
        let so = write_so(&state, b"stale");
        enable_in_config(&mut *state.config.lock().await, V84);
        let dl = FakeDownloader::serving_84();
        let runner = FakeRunner::new(true);
        let guard = state.php_settings_mutate.lock().await;
        let (replaced, ()) = tokio::join!(refresh(&state, &dl, &runner), async {
            assert_eq!(runner.calls(), 1);
            state.shutdown_tx.send_replace(true);
            drop(guard);
        });
        assert!(replaced.is_empty());
        assert_eq!(std::fs::read(&so).unwrap(), b"stale");
        assert!(temp_files(&state).is_empty());
    }

    #[test]
    fn restart_decision_table() {
        let a = Some(XdebugLoad {
            path: "/d/xdebug.so".into(),
            defines: vec![("xdebug.mode".into(), "debug".into())],
        });
        let b = Some(XdebugLoad {
            path: "/d/xdebug.so".into(),
            defines: vec![("xdebug.mode".into(), "profile".into())],
        });
        let cases = [
            (None, None, false, false),
            (None, a.clone(), false, true),
            (a.clone(), None, false, true),
            (a.clone(), a.clone(), false, false),
            (a.clone(), a.clone(), true, true),
            (a.clone(), b.clone(), false, true),
        ];
        for (old, new, fetched, want) in cases {
            assert_eq!(
                should_restart(old.as_ref(), new.as_ref(), fetched),
                want,
                "{old:?} {new:?} {fetched}"
            );
        }
    }

    #[test]
    fn load_map_excludes_disabled_legacy_and_conflicting_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = crate::test_support::dirs_in(tmp.path());
        let mut cfg = yerd_config::Config::default();
        let on = XdebugSettings {
            enabled: true,
            modes: vec![XdebugMode::Debug, XdebugMode::Profile],
            start: XdebugStart::Trigger,
        };
        cfg.php.xdebug.insert(V84, on.clone());
        cfg.php.xdebug.insert(PhpVersion::new(8, 3), on.clone());
        cfg.php.xdebug.insert(PhpVersion::new(7, 4), on.clone());
        cfg.php.xdebug.insert(
            PhpVersion::new(8, 2),
            XdebugSettings {
                enabled: false,
                ..on.clone()
            },
        );
        cfg.php.extensions.insert(
            PhpVersion::new(8, 3),
            vec![yerd_config::ExtEntry {
                name: "xdebug".into(),
                path: "/opt/x.so".into(),
                zend: true,
            }],
        );
        let map = xdebug_load_map(&cfg, &dirs);
        assert_eq!(map.keys().copied().collect::<Vec<_>>(), vec![V84]);
        let load = map.get(&V84).unwrap();
        assert_eq!(load.path, crate::ext_install::xdebug_so_path(&dirs, V84));
        let out_dir = output_dir(&dirs).to_string_lossy().into_owned();
        let want: Vec<(String, String)> = php_xdebug::managed_directives(&on, &out_dir)
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect();
        assert_eq!(load.defines, want);
        let installed = [PhpVersion::new(8, 3), V84, PhpVersion::new(7, 4)];
        assert_eq!(
            enabled_versions(&cfg, &installed),
            vec![PhpVersion::new(8, 3), V84]
        );
        assert_eq!(enabled_versions(&cfg, &[V84]), vec![V84]);
        assert!(enabled_versions(&cfg, &[]).is_empty());
    }

    #[test]
    fn startup_creates_the_output_dir_only_when_an_enabled_version_writes_output() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = crate::test_support::dirs_in(tmp.path());
        let mut cfg = yerd_config::Config::default();
        let with = |enabled: bool, modes: Vec<XdebugMode>| XdebugSettings {
            enabled,
            modes,
            start: XdebugStart::Always,
        };
        cfg.php
            .xdebug
            .insert(V84, with(true, vec![XdebugMode::Debug]));
        cfg.php.xdebug.insert(
            PhpVersion::new(8, 3),
            with(false, vec![XdebugMode::Profile]),
        );
        cfg.php
            .xdebug
            .insert(PhpVersion::new(7, 4), with(true, vec![XdebugMode::Trace]));
        ensure_output_dir(&cfg, &dirs);
        assert!(!output_dir(&dirs).exists());

        cfg.php
            .xdebug
            .insert(V84, with(true, vec![XdebugMode::Gcstats]));
        assert!(needs_output_dir(&cfg));
        ensure_output_dir(&cfg, &dirs);
        assert!(output_dir(&dirs).is_dir());
    }

    #[tokio::test]
    async fn php_versions_response_omits_a_hand_edited_default_xdebug_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        *state.config.lock().await = yerd_config::Config::from_toml(
            "version = 29\n[php]\ndefault = \"8.4\"\n\
             [php.xdebug.\"8.4\"]\nenabled = false\nmodes = [\"debug\"]\nstart = \"always\"\n",
        )
        .unwrap();
        match crate::ipc_server::php_versions_response(&state).await {
            Response::PhpVersions { xdebug, .. } => assert!(xdebug.is_empty(), "{xdebug:?}"),
            other => panic!("expected PhpVersions, got {other:?}"),
        }
    }

    #[test]
    fn debug_ini_sits_next_to_the_so() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = crate::test_support::dirs_in(tmp.path());
        assert!(debug_ini_path(&dirs, V84).ends_with("php-ext/php-8.4/debug.ini"));
    }
}
