//! `Paths` trait + [`PlatformDirs`] struct.

use std::path::PathBuf;

use crate::PlatformError;

/// The five directories Yerd cares about. Returned by [`Paths::resolve`].
///
/// **Existence is not guaranteed.** Callers are responsible for
/// `std::fs::create_dir_all` before writing into any of these paths.
/// `runtime` is security-sensitive on Linux when the fallback to
/// `/tmp/yerd-$UID` kicks in - caller should `mkdir(mode=0o700)` and, if
/// the directory already exists, verify ownership (`uid == geteuid()`)
/// and mode (`0o700`) before using it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformDirs {
    /// User-owned config directory (e.g. `~/.config/yerd` on Linux).
    pub config: PathBuf,
    /// User-owned persistent data directory (CA + leaf certs).
    pub data: PathBuf,
    /// User-owned long-lived state. Distinct from `data` on Linux
    /// (`XDG_STATE_HOME`); on macOS it coincides with `data`.
    pub state: PathBuf,
    /// User-owned cache directory (logs, downloads).
    pub cache: PathBuf,
    /// Runtime directory for the IPC socket and PID file.
    ///
    /// Linux: `XDG_RUNTIME_DIR/yerd` or, when `XDG_RUNTIME_DIR` is unset,
    /// `/tmp/yerd-$UID` (see struct-level docs for the caller contract).
    /// macOS: a deterministic `/tmp/yerd-$UID`, not `$TMPDIR`/`temp_dir()`.
    /// `os::macos::resolve` explains why the uid-derived path is load-bearing
    /// for socket reconstruction.
    /// Windows: `%TEMP%\yerd`, which is per-user (unlike Linux's shared `/tmp`),
    /// so no sticky-bit ownership check is needed there.
    pub runtime: PathBuf,
}

impl PlatformDirs {
    /// Resolve the per-user directories for an explicit `home` + `uid`, without
    /// reading `$HOME` or the XDG environment.
    ///
    /// [`Paths::resolve`] derives everything from the process environment, which
    /// is wrong under `sudo` (it points at root). `yerd uninstall`, run as root,
    /// uses this to target the *invoking* user's dirs instead. The layout
    /// reproduces exactly what `directories` produces in `resolve` (a
    /// drift-guard test asserts the two agree for the current home).
    ///
    /// `runtime` is the deterministic uid fallback (`/tmp/yerd-$uid`); a caller
    /// that also wants the `XDG_RUNTIME_DIR` location (`/run/user/$uid/yerd`,
    /// which `resolve` prefers when the env var is set) must add it itself,
    /// since the real value can't be recovered from a stripped sudo environment.
    ///
    /// On Windows `uid` is meaningless and ignored: the caller identifies the
    /// user by `home` alone. The layout is derived from `home` without env reads
    /// (the documented contract of `for_user`), reconstructing the *default
    /// profile shape* (`home\AppData\Roaming\yerd`, `home\AppData\Local\yerd`).
    /// Redirected or roaming profiles may differ from the live `resolve()` env
    /// answer, the same caveat class as the `XDG_RUNTIME_DIR` gap on Linux.
    #[must_use]
    pub fn for_user(home: &std::path::Path, uid: u32) -> Self {
        #[cfg(target_os = "macos")]
        {
            let runtime = PathBuf::from(format!("/tmp/yerd-{uid}"));
            let app = home
                .join("Library")
                .join("Application Support")
                .join("io.yerd.Yerd");
            let cache = home.join("Library").join("Caches").join("io.yerd.Yerd");
            Self {
                config: app.clone(),
                data: app.clone(),
                state: app,
                cache,
                runtime,
            }
        }
        #[cfg(target_os = "linux")]
        {
            let runtime = PathBuf::from(format!("/tmp/yerd-{uid}"));
            Self {
                config: home.join(".config").join("yerd"),
                data: home.join(".local").join("share").join("yerd"),
                state: home.join(".local").join("state").join("yerd"),
                cache: home.join(".cache").join("yerd"),
                runtime,
            }
        }
        #[cfg(target_os = "windows")]
        {
            let _ = uid;
            let roaming = home.join("AppData").join("Roaming").join("yerd");
            let local = home.join("AppData").join("Local").join("yerd");
            Self {
                config: roaming,
                data: local.join("data"),
                state: local.join("state"),
                cache: local.join("cache"),
                runtime: home.join("AppData").join("Local").join("Temp").join("yerd"),
            }
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        {
            let runtime = PathBuf::from(format!("/tmp/yerd-{uid}"));
            Self {
                config: home.join(".config").join("yerd"),
                data: home.join(".local").join("share").join("yerd"),
                state: home.join(".local").join("state").join("yerd"),
                cache: home.join(".cache").join("yerd"),
                runtime,
            }
        }
    }
}

/// OS path-discovery abstraction.
pub trait Paths {
    /// Resolve every directory in one call.
    ///
    /// The daemon calls this once at startup. The returned paths are not
    /// guaranteed to exist on disk; the caller is responsible for
    /// `create_dir_all` before writing into them.
    fn resolve(&self) -> Result<PlatformDirs, PlatformError>;
}

#[cfg(all(
    test,
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod for_user_tests {
    use super::PlatformDirs;
    use crate::{ActivePaths, Paths};

    /// `for_user` must reproduce the home-derived dirs that `resolve` produces -
    /// guards against the macOS fragment (`io.yerd.Yerd` vs bare `Yerd`) or the
    /// Windows `AppData` shape silently drifting. `runtime` is uid/env-derived
    /// and handled separately, so it is not compared.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn for_user_layout_matches_resolve_for_current_home() {
        let Some(home) = std::env::var_os("HOME") else {
            return;
        };
        #[cfg(not(target_os = "macos"))]
        for v in [
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
            "XDG_STATE_HOME",
        ] {
            if std::env::var_os(v).is_some() {
                return;
            }
        }
        let home = std::path::PathBuf::from(home);
        let r = ActivePaths::new().resolve().expect("resolve current dirs");
        let f = PlatformDirs::for_user(&home, 0);
        assert_eq!(f.config, r.config, "config dir");
        assert_eq!(f.data, r.data, "data dir");
        assert_eq!(f.cache, r.cache, "cache dir");
        assert_eq!(f.state, r.state, "state dir");
    }

    /// The exact Windows default-profile layout from a fake home.
    #[cfg(target_os = "windows")]
    #[test]
    fn for_user_windows_layout() {
        let home = std::path::Path::new(r"C:\Users\test");
        let f = PlatformDirs::for_user(home, 0);
        assert_eq!(f.config, home.join(r"AppData\Roaming\yerd"));
        assert_eq!(f.data, home.join(r"AppData\Local\yerd\data"));
        assert_eq!(f.state, home.join(r"AppData\Local\yerd\state"));
        assert_eq!(f.cache, home.join(r"AppData\Local\yerd\cache"));
        assert_eq!(f.runtime, home.join(r"AppData\Local\Temp\yerd"));
    }

    /// Windows drift-guard: when `%APPDATA%`/`%LOCALAPPDATA%` sit under
    /// `%USERPROFILE%` in the default shape, `for_user(USERPROFILE, 0)` must
    /// agree with the live `resolve()` for config/data/state/cache. Redirected
    /// profiles are skipped (mirrors the Unix XDG skip).
    #[cfg(target_os = "windows")]
    #[test]
    fn for_user_windows_matches_resolve_for_default_profile() {
        let (Some(profile), Some(appdata), Some(local)) = (
            std::env::var_os("USERPROFILE"),
            std::env::var_os("APPDATA"),
            std::env::var_os("LOCALAPPDATA"),
        ) else {
            return;
        };
        let profile = std::path::PathBuf::from(profile);
        let default_appdata = profile.join("AppData").join("Roaming");
        let default_local = profile.join("AppData").join("Local");
        if std::path::Path::new(&appdata) != default_appdata
            || std::path::Path::new(&local) != default_local
        {
            return;
        }
        let r = ActivePaths::new().resolve().expect("resolve current dirs");
        let f = PlatformDirs::for_user(&profile, 0);
        assert_eq!(f.config, r.config, "config dir");
        assert_eq!(f.data, r.data, "data dir");
        assert_eq!(f.cache, r.cache, "cache dir");
        assert_eq!(f.state, r.state, "state dir");
    }
}
