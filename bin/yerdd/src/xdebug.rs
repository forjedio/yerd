//! The shared set-Xdebug operation behind `Request::SetXdebug` (CLI, GUI, MCP).
//!
//! The settings model lives in [`yerd_core::php_xdebug`]; this module only
//! orchestrates: resolve the version, merge the request, fetch and load-probe the
//! bundled `xdebug.so` when it is being switched on, persist, then push the new
//! load map into the `PhpManager` and restart that one pool if its command line
//! changed.
//!
//! Lock discipline matches `set_php_directives`: no lock is held across the
//! download or the probe; `php_settings_mutate` is taken for persist + apply, and
//! the config lock is dropped before the manager lock is taken.

use std::collections::BTreeMap;
use std::path::Path;

use yerd_core::php_xdebug::{self, XdebugMode, XdebugSettings, XdebugStart};
use yerd_core::PhpVersion;
use yerd_ipc::{ErrorCode, Response};
use yerd_php::{CommandRunner, Downloader, XdebugLoad};
use yerd_platform::PlatformDirs;

use crate::state::DaemonState;

/// One `SetXdebug` request's fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XdebugRequest {
    /// The PHP version; `None` means the configured default.
    pub version: Option<PhpVersion>,
    /// Whether the version's pool should load Xdebug.
    pub enabled: bool,
    /// New modes, or `None` to keep the current ones.
    pub modes: Option<Vec<XdebugMode>>,
    /// New start trigger, or `None` to keep the current one.
    pub start: Option<XdebugStart>,
}

/// What a successful [`set_xdebug_with`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applied {
    /// Nothing changed: no save, no fetch, no restart.
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
    let (os, arch) = crate::ext_install::host_os_arch();
    let runner = yerd_php::TokioCommandRunner;
    match set_xdebug_with(state, &dl, &runner, os, arch, req).await {
        Ok(_) => crate::ipc_server::php_versions_response(state).await,
        Err(resp) => resp,
    }
}

/// Injectable core of [`set_xdebug`]: the downloader, probe runner and host
/// triple are parameters so tests run offline on any CI host.
///
/// # Errors
/// The error `Response` to send back (typed `ErrorCode`, human message).
pub async fn set_xdebug_with(
    state: &DaemonState,
    dl: &dyn Downloader,
    runner: &dyn CommandRunner,
    os: &str,
    arch: &str,
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
    if new == current && (!new.enabled || so.is_file()) {
        return Ok(Applied::Unchanged);
    }

    let fetched_this_call = if new.enabled && (!current.enabled || !so.is_file()) {
        if let Some(name) = registered {
            return Err(conflict(v, &name));
        }
        fetch_and_probe(state, dl, runner, (os, arch), v, &so).await?
    } else {
        false
    };
    if new.enabled && new.writes_output() {
        let dir = output_dir(&state.dirs);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!(path = %dir.display(), error = %e, "failed to create the Xdebug output dir");
        }
    }

    let _mutate_guard = state.php_settings_mutate.lock().await;
    let (new_map, old_load, new_load) = persist(state, v, &req).await?;
    let restart = apply_xdebug(
        state,
        v,
        new_map,
        old_load.as_ref(),
        new_load.as_ref(),
        fetched_this_call,
    )
    .await;
    tracing::info!(version = %v, restart, "applied Xdebug settings");
    Ok(Applied::Changed { restart })
}

/// Make the `.so` current (downloading only when it is missing or its
/// SHA-256 no longer matches the manifest), then load-probe it. Returns whether
/// this call replaced the file. When the manifest or download fails but a file
/// is already on disk, that file is used so an offline enable still works.
/// Holds no lock.
async fn fetch_and_probe(
    state: &DaemonState,
    dl: &dyn Downloader,
    runner: &dyn CommandRunner,
    (os, arch): (&str, &str),
    v: PhpVersion,
    so: &Path,
) -> Result<bool, Response> {
    let fetched = match crate::ext_install::ensure_xdebug(&state.dirs, dl, v, os, arch).await {
        Ok(replaced) => replaced,
        Err(e) if so.is_file() => {
            tracing::warn!(version = %v, error = %e, "couldn't refresh Xdebug, using the existing file");
            false
        }
        Err(e) => {
            return Err(error(
                ErrorCode::XdebugUnavailable,
                format!("couldn't download Xdebug for PHP {v}: {e}"),
            ))
        }
    };
    let php_bin = crate::php_install::cli_binary_path(&state.dirs, v);
    if let Err(e) = yerd_php::probe_extension(runner, &php_bin, so, true).await {
        return Err(error(
            ErrorCode::ExtensionLoadFailed,
            format!("Xdebug failed to load into PHP {v}: {e}"),
        ));
    }
    Ok(fetched)
}

/// Startup refresh: make the bundled `xdebug.so` current for every installed
/// version with Xdebug enabled, then restart the running pool of each version
/// whose file was replaced. Without the restart, a pool lazily started before
/// the refresh finished would keep running without (or with a stale) Xdebug
/// while status reports it on. Returns the replaced versions.
///
/// Same lock discipline as [`set_xdebug_with`]: no lock across the download;
/// `php_settings_mutate` is held per restart, and the config lock is dropped
/// before the manager lock is taken.
pub async fn refresh_enabled(
    state: &DaemonState,
    dl: &dyn Downloader,
    os: &str,
    arch: &str,
) -> Vec<PhpVersion> {
    let installed = crate::ext_install::installed_versions(&state.dirs);
    let versions = enabled_versions(&*state.config.lock().await, &installed);
    let replaced =
        crate::ext_install::refresh_xdebug_for(&state.dirs, dl, &versions, os, arch).await;
    for v in &replaced {
        let _mutate_guard = state.php_settings_mutate.lock().await;
        let map = xdebug_load_map(&*state.config.lock().await, &state.dirs);
        let Some(load) = map.get(v).cloned() else {
            continue;
        };
        apply_xdebug(state, *v, map, Some(&load), Some(&load), true).await;
        tracing::info!(version = %v, "refreshed Xdebug and restarted its pool if running");
    }
    replaced
}

/// The new load map plus `v`'s load before and after.
type Loads = (
    BTreeMap<PhpVersion, XdebugLoad>,
    Option<XdebugLoad>,
    Option<XdebugLoad>,
);

/// Re-check and persist the request against the fresh config under the config
/// lock (the caller holds `php_settings_mutate`). A version whose settings end
/// up equal to the default has its entry removed rather than stored.
async fn persist(
    state: &DaemonState,
    v: PhpVersion,
    req: &XdebugRequest,
) -> Result<Loads, Response> {
    let mut cfg_guard = state.config.lock().await;
    let old_load = xdebug_load_map(&cfg_guard, &state.dirs).remove(&v);
    if !crate::php_install::cli_binary_path(&state.dirs, v).exists() {
        return Err(error(
            ErrorCode::NotFound,
            format!("PHP {v} is not installed - run `yerd install php {v}`"),
        ));
    }
    let fresh = cfg_guard.php.xdebug.get(&v).cloned().unwrap_or_default();
    let merged = merge(&fresh, req)?;
    if merged.enabled {
        if let Some(name) = registered_xdebug(&cfg_guard, v) {
            return Err(conflict(v, &name));
        }
    }
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
    let new_map = xdebug_load_map(&cfg_guard, &state.dirs);
    let new_load = new_map.get(&v).cloned();
    Ok((new_map, old_load, new_load))
}

/// Push `new_map` into the manager and restart `v`'s pool when
/// [`should_restart`] says so and the pool has been started. Returns the
/// restart decision.
async fn apply_xdebug(
    state: &DaemonState,
    v: PhpVersion,
    new_map: BTreeMap<PhpVersion, XdebugLoad>,
    old_load: Option<&XdebugLoad>,
    new_load: Option<&XdebugLoad>,
    fetched_this_call: bool,
) -> bool {
    let restart = should_restart(old_load, new_load, fetched_this_call);
    let mut mgr = state.php_manager.lock().await;
    mgr.set_xdebug(new_map);
    if restart && mgr.snapshots().iter().any(|s| s.version == v) {
        if let Err(e) = mgr.restart(v).await {
            tracing::warn!(version = %v, error = %e, "failed to restart FPM pool after Xdebug change");
        }
    }
    restart
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

/// `{data}/xdebug`, where profile and trace files are written.
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
        .map_err(|e| error(ErrorCode::InvalidPath, e.to_string()))
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
        calls: AtomicUsize,
    }

    impl FakeRunner {
        fn new(ok: bool) -> Self {
            Self {
                ok,
                calls: AtomicUsize::new(0),
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
            Ok(yerd_php::ProbeOutput {
                status_ok: self.ok,
                stdout: "[Zend Modules]\nXdebug\n".to_owned(),
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
            enabled,
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
        set_xdebug_with(state, dl, runner, "macos", "aarch64", r).await
    }

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
            enabled: false,
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
            "macos",
            "x86_64",
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
            enabled: true,
            modes: Some(Vec::new()),
            start: None,
        };
        let err = run(&state, &dl, &runner, r).await.unwrap_err();
        assert_eq!(error_code(err), ErrorCode::InvalidPath);
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
        let replaced = refresh_enabled(&state, &dl, "macos", "aarch64").await;
        assert_eq!(replaced, vec![V84]);
        assert_eq!(std::fs::read(&so).unwrap(), SO_BYTES);
        assert_eq!(dl.calls(), 2);
        assert!(!crate::ext_install::xdebug_so_path(&state.dirs, PhpVersion::new(8, 3)).exists());

        let again = refresh_enabled(&state, &dl, "macos", "aarch64").await;
        assert!(again.is_empty());
        assert_eq!(dl.calls(), 3);
    }

    #[tokio::test]
    async fn startup_refresh_without_installed_enabled_versions_makes_no_calls() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        enable_in_config(&mut *state.config.lock().await, V84);
        let dl = FakeDownloader::serving_84();
        let replaced = refresh_enabled(&state, &dl, "macos", "aarch64").await;
        assert!(replaced.is_empty());
        assert_eq!(dl.calls(), 0);
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
    fn debug_ini_sits_next_to_the_so() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = crate::test_support::dirs_in(tmp.path());
        assert!(debug_ini_path(&dirs, V84).ends_with("php-ext/php-8.4/debug.ini"));
    }
}
