//! Per-pool FPM configuration values.
//!
//! A [`PoolConfig`] is the input to [`crate::pure::fpm_conf::render_fpm_conf`].
//! It's `#[non_exhaustive]` so future per-pool knobs (worker counts,
//! per-pool env vars, …) can be added without breaking downstream code.

use std::path::PathBuf;

use yerd_core::PhpVersion;
use yerd_platform::PlatformDirs;

use crate::listen::Listen;

/// The settings driving one rendered FPM pool config file.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolConfig {
    /// PHP version this pool serves (used to compute the pool name and
    /// FPM binary).
    pub version: PhpVersion,
    /// Where FPM listens for requests.
    pub listen: Listen,
    /// File FPM writes its master PID to.
    pub pid_file: PathBuf,
    /// File FPM writes worker stdout/stderr + its own error log to.
    pub error_log: PathBuf,
    /// File the manager writes the rendered FPM config to (so FPM can
    /// read it back via `--fpm-config`).
    pub config_path: PathBuf,
    /// FPM process manager mode (`static`, `dynamic`, or `ondemand`).
    pub pm: ProcessManagerMode,
    /// FPM `pm.max_children`.
    pub max_children: u32,
    /// Global PHP ini directives to apply, as `(name, value)` pairs sorted by
    /// name. Rendered as `php_value[name] = value` / `php_flag[name] = value`
    /// (per [`yerd_core::php_settings::directive`]). Empty by default.
    pub ini: Vec<(String, String)>,
    /// Optional PHP extension to load via the FPM command line
    /// (`-d extension=<path>`). Loaded at PHP startup (MINIT); used for the
    /// daemon-managed dump-telemetry extension. `None` = none.
    pub extension: Option<PathBuf>,
    /// Extra INI directives passed on the FPM command line (`-d key=value`),
    /// applied only when an [`Self::extension`] is set (e.g. the extension's
    /// state-file path). Kept off the pool config file deliberately.
    pub ini_defines: Vec<(String, String)>,
    /// Managed CA bundle the bundled PHP verifies TLS against, rendered as
    /// `php_admin_value[openssl.cafile]` / `php_admin_value[curl.cainfo]` so
    /// PHP trusts the Yerd CA on `.test` HTTPS. Daemon-controlled (not a user
    /// setting); `None` leaves PHP's compiled-in default untouched.
    pub ca_bundle: Option<PathBuf>,
}

/// FPM process-manager mode.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessManagerMode {
    /// Pre-fork a fixed number of children.
    Static,
    /// Dynamic worker pool with min/max spare workers.
    Dynamic,
    /// Spawn workers on demand; idle them after a timeout.
    OnDemand,
}

impl PoolConfig {
    /// Build a sane local-development pool config.
    ///
    /// - `pm = OnDemand`
    /// - `max_children = 16` (accommodates Laravel + Vite + several tabs).
    /// - Pid + log under `dirs.state`, config under `dirs.config`.
    /// - All basenames embed `version` AND `instance_id` so concurrent
    ///   Yerd daemons on the same host don't clobber each other.
    #[must_use]
    pub fn dev_defaults(
        version: PhpVersion,
        listen: Listen,
        dirs: &PlatformDirs,
        instance_id: u32,
    ) -> Self {
        Self {
            version,
            listen,
            pid_file: dirs.state.join(format!("fpm-{version}-{instance_id}.pid")),
            error_log: dirs.state.join(format!("fpm-{version}-{instance_id}.log")),
            config_path: dirs
                .config
                .join(format!("php-fpm-{version}-{instance_id}.conf")),
            pm: ProcessManagerMode::OnDemand,
            max_children: 16,
            ini: Vec::new(),
            extension: None,
            ini_defines: Vec::new(),
            ca_bundle: None,
        }
    }
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

    #[test]
    fn dev_defaults_embeds_version_and_instance() {
        let dirs = PlatformDirs {
            config: PathBuf::from("/cfg"),
            data: PathBuf::from("/data"),
            state: PathBuf::from("/state"),
            cache: PathBuf::from("/cache"),
            runtime: PathBuf::from("/run"),
        };
        let v = PhpVersion::new(8, 3);
        let listen = Listen::UnixSocket(PathBuf::from("/run/x.sock"));
        let cfg = PoolConfig::dev_defaults(v, listen, &dirs, 4242);
        assert_eq!(cfg.pid_file, PathBuf::from("/state/fpm-8.3-4242.pid"));
        assert_eq!(cfg.error_log, PathBuf::from("/state/fpm-8.3-4242.log"));
        assert_eq!(cfg.config_path, PathBuf::from("/cfg/php-fpm-8.3-4242.conf"));
        assert_eq!(cfg.pm, ProcessManagerMode::OnDemand);
        assert_eq!(cfg.max_children, 16);
    }
}
