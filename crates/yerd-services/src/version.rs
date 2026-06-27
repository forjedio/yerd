//! Service version identifiers, on-disk layout, and install discovery.
//!
//! A [`ServiceVersion`] is an opaque, validated label (e.g. `"8"` for Valkey 8,
//! `"8.4"` for `MySQL`, `"16"` for Postgres) - services don't share PHP's
//! `major.minor` structure, so it stays a string. The path helpers and
//! [`discover_installed`] define the on-disk layout under `dirs.data/services`
//! and `dirs.state/services`.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use yerd_platform::PlatformDirs;

use crate::error::ServiceError;
use crate::service::Service;

/// Filename of the installed-version marker inside a per-version install dir.
pub const VERSION_MARKER: &str = ".yerd-version";

/// A validated service version label.
///
/// Validation keeps it safe to use as a single path component: non-empty, and
/// only ASCII alphanumerics plus `.`, `_`, `-` (no separators, no `..`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ServiceVersion(String);

impl ServiceVersion {
    /// Borrow the underlying string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The major component (the substring before the first `.`), used to pin
    /// datadirs for engines whose on-disk format is major-incompatible.
    #[must_use]
    pub fn major(&self) -> &str {
        self.0.split('.').next().unwrap_or(&self.0)
    }

    fn is_valid(s: &str) -> bool {
        !s.is_empty()
            && s != "."
            && s != ".."
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    }
}

impl FromStr for ServiceVersion {
    type Err = ServiceError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if Self::is_valid(s) {
            Ok(ServiceVersion(s.to_owned()))
        } else {
            Err(ServiceError::UnsupportedPlatform {
                detail: format!("invalid service version label {s:?}"),
            })
        }
    }
}

impl fmt::Display for ServiceVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Root under which all service installs live: `data/services`.
#[must_use]
pub fn services_root(dirs: &PlatformDirs) -> PathBuf {
    dirs.data.join("services")
}

/// Per-service install root: `data/services/<id>`.
#[must_use]
pub fn service_root(dirs: &PlatformDirs, service: Service) -> PathBuf {
    services_root(dirs).join(service.id())
}

/// A specific version's install dir: `data/services/<id>/<version>`.
#[must_use]
pub fn install_dir(dirs: &PlatformDirs, service: Service, version: &ServiceVersion) -> PathBuf {
    service_root(dirs, service).join(version.as_str())
}

/// Absolute path to a version's server binary:
/// `data/services/<id>/<version>/bin/<server_binary>`.
#[must_use]
pub fn server_path(dirs: &PlatformDirs, service: Service, version: &ServiceVersion) -> PathBuf {
    install_dir(dirs, service, version)
        .join("bin")
        .join(service.server_binary())
}

/// The datadir for a service+version. Pinned per *major* for engines whose
/// on-disk format is major-incompatible (Postgres); otherwise a single shared
/// datadir per engine.
#[must_use]
pub fn datadir(dirs: &PlatformDirs, service: Service, version: &ServiceVersion) -> PathBuf {
    let root = service_root(dirs, service);
    if service.datadir_pinned_to_major() {
        root.join(format!("data-{}", version.major()))
    } else {
        root.join("data")
    }
}

/// The rendered config-file path: `state/services/<id>/<id>.conf`.
#[must_use]
pub fn config_path(dirs: &PlatformDirs, service: Service) -> PathBuf {
    dirs.state
        .join("services")
        .join(service.id())
        .join(format!("{}.conf", service.id()))
}

/// The `MySQL`/`MariaDB` bootstrap-SQL path: `state/services/<id>/<id>-init.sql`.
///
/// Referenced by the `init-file` directive in the rendered `my.cnf`; the server
/// runs it on every start. Lives beside the config (rewritten each start), not in
/// the datadir, so it persists independently of re-initialisation.
#[must_use]
pub fn init_file_path(dirs: &PlatformDirs, service: Service) -> PathBuf {
    dirs.state
        .join("services")
        .join(service.id())
        .join(format!("{}-init.sql", service.id()))
}

/// The server log-file path: `state/services/<id>/<id>.log`.
#[must_use]
pub fn log_path(dirs: &PlatformDirs, service: Service) -> PathBuf {
    dirs.state
        .join("services")
        .join(service.id())
        .join(format!("{}.log", service.id()))
}

/// The Unix-socket path for the `MySQL`/`MariaDB` server (and the client that
/// connects to it), under the short `runtime` dir to stay within the platform
/// `sun_path` length limit. Unused for engines that don't use a Unix socket.
#[must_use]
pub fn socket_path(dirs: &PlatformDirs, service: Service) -> PathBuf {
    dirs.runtime
        .join("services")
        .join(service.id())
        .join(format!("{}.sock", service.id()))
}

/// Discover every installed `(service, version)` by scanning
/// `data/services/<id>/` for version dirs that actually contain the server
/// binary. A missing services root yields an empty map (not an error); other
/// I/O errors propagate as [`ServiceError::DiscoveryIo`].
pub fn discover_installed(
    dirs: &PlatformDirs,
) -> Result<BTreeMap<Service, Vec<ServiceVersion>>, ServiceError> {
    let mut out: BTreeMap<Service, Vec<ServiceVersion>> = BTreeMap::new();
    for service in Service::ALL {
        let root = service_root(dirs, service);
        let entries = match std::fs::read_dir(&root) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(source) => return Err(ServiceError::DiscoveryIo { dir: root, source }),
        };
        let mut versions = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|source| ServiceError::DiscoveryIo {
                dir: root.clone(),
                source,
            })?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let Ok(version) = ServiceVersion::from_str(&name) else {
                continue;
            };
            if server_path(dirs, service, &version).is_file() {
                versions.push(version);
            }
        }
        if !versions.is_empty() {
            versions.sort();
            out.insert(service, versions);
        }
    }
    Ok(out)
}

/// The installed version's recorded marker string, or `None` if not installed.
#[must_use]
pub fn installed_marker(
    dirs: &PlatformDirs,
    service: Service,
    version: &ServiceVersion,
) -> Option<String> {
    let marker = install_dir(dirs, service, version).join(VERSION_MARKER);
    let v = std::fs::read_to_string(marker).ok()?;
    let v = v.trim().to_owned();
    if v.is_empty() {
        None
    } else {
        Some(v)
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

    fn dirs_in(tmp: &std::path::Path) -> PlatformDirs {
        PlatformDirs {
            config: tmp.join("c"),
            data: tmp.join("d"),
            state: tmp.join("s"),
            cache: tmp.join("ca"),
            runtime: tmp.join("r"),
        }
    }

    #[test]
    fn version_validation() {
        assert!(ServiceVersion::from_str("8").is_ok());
        assert!(ServiceVersion::from_str("8.4").is_ok());
        assert!(ServiceVersion::from_str("16-beta_1").is_ok());
        assert!(ServiceVersion::from_str("").is_err());
        assert!(ServiceVersion::from_str("..").is_err());
        assert!(ServiceVersion::from_str("a/b").is_err());
        assert!(ServiceVersion::from_str("a b").is_err());
    }

    #[test]
    fn major_component() {
        assert_eq!(ServiceVersion::from_str("8").unwrap().major(), "8");
        assert_eq!(ServiceVersion::from_str("8.4.1").unwrap().major(), "8");
        assert_eq!(ServiceVersion::from_str("16").unwrap().major(), "16");
    }

    #[test]
    fn datadir_pins_postgres_to_major_only() {
        let dirs = dirs_in(std::path::Path::new("/tmp/x"));
        let v = ServiceVersion::from_str("16.2").unwrap();
        assert_eq!(
            datadir(&dirs, Service::Postgres, &v),
            PathBuf::from("/tmp/x/d/services/postgres/data-16")
        );
        let v8 = ServiceVersion::from_str("8.4").unwrap();
        assert_eq!(
            datadir(&dirs, Service::MySql, &v8),
            PathBuf::from("/tmp/x/d/services/mysql/data")
        );
    }

    #[test]
    fn server_path_layout() {
        let dirs = dirs_in(std::path::Path::new("/tmp/x"));
        let v = ServiceVersion::from_str("8").unwrap();
        assert_eq!(
            server_path(&dirs, Service::Redis, &v),
            PathBuf::from("/tmp/x/d/services/redis/8/bin/valkey-server")
        );
    }

    #[test]
    fn discover_finds_installed_versions_only() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let v = ServiceVersion::from_str("8").unwrap();

        assert!(discover_installed(&dirs).unwrap().is_empty());

        std::fs::create_dir_all(install_dir(&dirs, Service::Redis, &v).join("bin")).unwrap();
        assert!(discover_installed(&dirs).unwrap().is_empty());

        std::fs::write(server_path(&dirs, Service::Redis, &v), b"#!/bin/sh\n").unwrap();
        let found = discover_installed(&dirs).unwrap();
        assert_eq!(found.get(&Service::Redis), Some(&vec![v]));
    }
}
