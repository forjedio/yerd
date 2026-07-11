//! Service version identifiers, on-disk layout, and install discovery.
//!
//! A [`ServiceVersion`] is an opaque, validated label (e.g. `"8"` for Valkey 8,
//! `"8.4"` for `MySQL`, `"16"` for Postgres) - services don't share PHP's
//! `major.minor` structure, so it stays a string. The path helpers and
//! [`discover_installed`] define the on-disk layout under `dirs.data/services`
//! and `dirs.state/services`.
//!
//! Paths are keyed by a service *type id* string (`"redis"`, `"postgres"`, ...)
//! plus the few per-type facts a path depends on (server-binary name,
//! datadir-pinned-to-major), which the caller reads from the type's
//! [`crate::service::ServiceDefinition`]. This keeps `version` free of the trait
//! object while the manager and daemon supply the facts.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use yerd_platform::PlatformDirs;

use crate::error::ServiceError;
use crate::service::ServiceRegistry;

/// Filename of the installed-version marker inside a per-version install dir.
pub const VERSION_MARKER: &str = ".yerd-version";

/// A validated service version label.
///
/// Validation keeps it safe to use as a single path component: non-empty, and
/// only ASCII alphanumerics plus `.`, `_`, `-` (no separators, no `..`).
///
/// Ordering is numeric per dotted component (see [`compare_version_labels`]) so
/// `"10.11" > "8.4"` and `"8" < "16"`, unlike byte-wise string order. This makes
/// the "latest" (`.last()` / `.pop()`) selectors across the daemon and CLI pick
/// the newest release for engines with multi-digit majors, not the
/// lexicographically-greatest label.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ServiceVersion(String);

impl Ord for ServiceVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        compare_version_labels(&self.0, &other.0).then_with(|| self.0.cmp(&other.0))
    }
}

impl PartialOrd for ServiceVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Compare two version labels numerically, component-by-component (split on `.`),
/// so `"10.11" > "8.4"` and `"8.4" > "8.0"`. Within a component a leading run of
/// ASCII digits is compared as a number; a missing trailing component counts as
/// `0`, so `"8" == "8.0"`.
///
/// When the numbers tie, a **plain** component (no `-`/`_` suffix) ranks above a
/// suffixed one, so `"16.10" > "16.10-full"` and the unsuffixed build is chosen
/// as latest. Two suffixed components fall back to lexical order.
///
/// Returns [`Ordering::Equal`] for numerically-equal labels that differ only in
/// spelling (`"8"` vs `"8.0"`); callers keep a byte-wise tiebreak after this so
/// the total order stays consistent with `Eq`.
fn compare_version_labels(a: &str, b: &str) -> Ordering {
    let mut a_parts = a.split('.');
    let mut b_parts = b.split('.');
    loop {
        match (a_parts.next(), b_parts.next()) {
            (None, None) => return Ordering::Equal,
            (a_comp, b_comp) => {
                let ord = compare_component(a_comp.unwrap_or("0"), b_comp.unwrap_or("0"));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
        }
    }
}

/// Compare one dotted component by its leading numeric run, then prefer the
/// suffix-free spelling (so `"10" > "10-full"`), falling back to lexical order
/// between two suffixed components.
fn compare_component(a: &str, b: &str) -> Ordering {
    let (a_num, a_rest) = split_numeric_prefix(a);
    let (b_num, b_rest) = split_numeric_prefix(b);
    a_num
        .cmp(&b_num)
        .then_with(|| compare_suffix(a_rest, b_rest))
}

/// Rank an empty suffix (a plain build) above any non-empty suffix; two
/// non-empty suffixes compare lexically.
fn compare_suffix(a: &str, b: &str) -> Ordering {
    match (a.is_empty(), b.is_empty()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) => a.cmp(b),
    }
}

/// Split a leading run of ASCII digits (parsed as a number, `0` if absent or
/// overflowing) from the remaining suffix.
fn split_numeric_prefix(s: &str) -> (u64, &str) {
    let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (digits, rest) = s.split_at(end);
    (digits.parse().unwrap_or(0), rest)
}

impl ServiceVersion {
    /// Borrow the underlying string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The major component: the leading run before the first `.` or `-`, used to
    /// pin datadirs for engines whose on-disk format is major-incompatible. A
    /// variant suffix (`<version>-<variant>`, e.g. the `PostGIS` `full` build) is
    /// ignored, so `"17-full"` / `"17.10-full"` share the major `17` with the base
    /// `"17"` / `"17.10"` builds - and thus the same datadir. Always non-empty: a
    /// valid label starts with an ASCII alphanumeric (see [`Self::is_valid`]).
    #[must_use]
    pub fn major(&self) -> &str {
        self.0.split(['.', '-']).next().unwrap_or(&self.0)
    }

    /// Whether this label carries a variant suffix (`"17-full"` yes, `"17.10"` no).
    /// The numeric version part never contains a hyphen, so a hyphen marks a variant.
    #[must_use]
    pub fn has_variant(&self) -> bool {
        self.0.contains('-')
    }

    /// A label is a safe single path component and yields a non-empty [`major`]:
    /// it must start with an ASCII alphanumeric (so no leading `.`/`-`/`_`, and no
    /// `.`/`..`) and contain only ASCII alphanumerics plus `.`, `_`, `-`.
    ///
    /// [`major`]: Self::major
    fn is_valid(s: &str) -> bool {
        s.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
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
pub fn service_root(dirs: &PlatformDirs, service_id: &str) -> PathBuf {
    services_root(dirs).join(service_id)
}

/// A specific version's install dir: `data/services/<id>/<version>`.
#[must_use]
pub fn install_dir(dirs: &PlatformDirs, service_id: &str, version: &ServiceVersion) -> PathBuf {
    service_root(dirs, service_id).join(version.as_str())
}

/// Absolute path to a version's server binary:
/// `data/services/<id>/<version>/bin/<server_binary>`.
#[must_use]
pub fn server_path(
    dirs: &PlatformDirs,
    service_id: &str,
    server_binary: &str,
    version: &ServiceVersion,
) -> PathBuf {
    install_dir(dirs, service_id, version)
        .join("bin")
        .join(server_binary)
}

/// The datadir for a service+version. Pinned per *major* when `pinned_to_major`
/// (engines whose on-disk format is major-incompatible, i.e. Postgres);
/// otherwise a single shared datadir per engine.
#[must_use]
pub fn datadir(
    dirs: &PlatformDirs,
    service_id: &str,
    pinned_to_major: bool,
    version: &ServiceVersion,
) -> PathBuf {
    let root = service_root(dirs, service_id);
    if pinned_to_major {
        root.join(format!("data-{}", version.major()))
    } else {
        root.join("data")
    }
}

/// The rendered config-file path: `state/services/<id>/<id>.conf`.
#[must_use]
pub fn config_path(dirs: &PlatformDirs, service_id: &str) -> PathBuf {
    dirs.state
        .join("services")
        .join(service_id)
        .join(format!("{service_id}.conf"))
}

/// The `MySQL`/`MariaDB` bootstrap-SQL path: `state/services/<id>/<id>-init.sql`.
///
/// Referenced by the `init-file` directive in the rendered `my.cnf`; the server
/// runs it on every start. Lives beside the config (rewritten each start), not in
/// the datadir, so it persists independently of re-initialisation.
#[must_use]
pub fn init_file_path(dirs: &PlatformDirs, service_id: &str) -> PathBuf {
    dirs.state
        .join("services")
        .join(service_id)
        .join(format!("{service_id}-init.sql"))
}

/// The server log-file path: `state/services/<id>/<id>.log`.
#[must_use]
pub fn log_path(dirs: &PlatformDirs, service_id: &str) -> PathBuf {
    dirs.state
        .join("services")
        .join(service_id)
        .join(format!("{service_id}.log"))
}

/// The per-instance log-file path, keyed by wire id. A single-instance engine
/// keeps `state/services/<id>/<id>.log`; a per-site instance (`"reverb:blog"`)
/// uses `state/services/<type>/<site>.log`, so each site's app server writes its
/// own log (and the daemon reads back the same file the manager wrote).
#[must_use]
pub fn instance_log_path(dirs: &PlatformDirs, wire_id: &str) -> PathBuf {
    match wire_id.split_once(':') {
        Some((ty, site)) => dirs
            .state
            .join("services")
            .join(ty)
            .join(format!("{site}.log")),
        None => log_path(dirs, wire_id),
    }
}

/// The Unix-socket path for the `MySQL`/`MariaDB` server (and the client that
/// connects to it), under the short `runtime` dir to stay within the platform
/// `sun_path` length limit. Unused for engines that don't use a Unix socket.
#[must_use]
pub fn socket_path(dirs: &PlatformDirs, service_id: &str) -> PathBuf {
    dirs.runtime
        .join("services")
        .join(service_id)
        .join(format!("{service_id}.sock"))
}

/// Discover every installed `(service id, version)` by scanning
/// `data/services/<id>/` for version dirs that actually contain the server
/// binary. Only versioned types in `registry` are scanned. A missing services
/// root yields an empty map (not an error); other I/O errors propagate as
/// [`ServiceError::DiscoveryIo`].
pub fn discover_installed(
    dirs: &PlatformDirs,
    registry: &ServiceRegistry,
) -> Result<BTreeMap<String, Vec<ServiceVersion>>, ServiceError> {
    let mut out: BTreeMap<String, Vec<ServiceVersion>> = BTreeMap::new();
    for def in registry.iter() {
        if !def.requires_version() {
            continue;
        }
        let Some(server_binary) = def.server_binary() else {
            continue;
        };
        let id = def.id();
        let root = service_root(dirs, id);
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
            if server_path(dirs, id, server_binary, &version).is_file() {
                versions.push(version);
            }
        }
        if !versions.is_empty() {
            versions.sort();
            out.insert(id.to_owned(), versions);
        }
    }
    Ok(out)
}

/// The installed version's recorded marker string, or `None` if not installed.
#[must_use]
pub fn installed_marker(
    dirs: &PlatformDirs,
    service_id: &str,
    version: &ServiceVersion,
) -> Option<String> {
    let marker = install_dir(dirs, service_id, version).join(VERSION_MARKER);
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
        assert!(ServiceVersion::from_str("-full").is_err());
        assert!(ServiceVersion::from_str(".17").is_err());
    }

    #[test]
    fn ordering_is_numeric_not_lexicographic() {
        let v = |s: &str| ServiceVersion::from_str(s).unwrap();

        assert!(v("10.11") > v("8.4"));
        assert!(v("16") > v("9"));
        assert!(v("8.4") > v("8.0"));
        assert!(v("11.4") > v("10.11"));
        assert_eq!(v("8").cmp(&v("8.0")), Ordering::Less);
        assert!(v("8.0") > v("8"));
    }

    #[test]
    fn plain_build_outranks_suffixed_at_same_number() {
        let v = |s: &str| ServiceVersion::from_str(s).unwrap();

        assert!(v("16.10") > v("16.10-full"));
        assert!(v("16.10") > v("16.10-slim"));
        assert!(v("16.11") > v("16.10-full"));
        assert!(v("16.10-full") > v("16.10-alpha"));

        let mut vs: Vec<ServiceVersion> = ["16.10-full", "16.10", "16.9"]
            .iter()
            .map(|s| v(s))
            .collect();
        vs.sort();
        assert_eq!(vs.last().map(ServiceVersion::as_str), Some("16.10"));
    }

    #[test]
    fn sort_puts_newest_last() {
        let mut vs: Vec<ServiceVersion> = ["8.4", "10.11", "10.5", "11.4"]
            .iter()
            .map(|s| ServiceVersion::from_str(s).unwrap())
            .collect();
        vs.sort();
        let labels: Vec<&str> = vs.iter().map(ServiceVersion::as_str).collect();
        assert_eq!(labels, ["8.4", "10.5", "10.11", "11.4"]);
        assert_eq!(vs.last().map(ServiceVersion::as_str), Some("11.4"));
    }

    #[test]
    fn major_component() {
        assert_eq!(ServiceVersion::from_str("8").unwrap().major(), "8");
        assert_eq!(ServiceVersion::from_str("8.4.1").unwrap().major(), "8");
        assert_eq!(ServiceVersion::from_str("16").unwrap().major(), "16");
    }

    /// A variant suffix (`-full` or any other) is ignored by `major()`, so a
    /// variant shares the numeric major - and thus the datadir - of its base.
    #[test]
    fn major_ignores_variant_suffix() {
        assert_eq!(ServiceVersion::from_str("17-full").unwrap().major(), "17");
        assert_eq!(
            ServiceVersion::from_str("17.10-full").unwrap().major(),
            "17"
        );
        assert_eq!(ServiceVersion::from_str("17-foo").unwrap().major(), "17");
    }

    #[test]
    fn has_variant_keys_on_a_hyphen() {
        assert!(ServiceVersion::from_str("17-full").unwrap().has_variant());
        assert!(ServiceVersion::from_str("16.10-full")
            .unwrap()
            .has_variant());
        assert!(!ServiceVersion::from_str("17").unwrap().has_variant());
        assert!(!ServiceVersion::from_str("8.4").unwrap().has_variant());
        assert!(!ServiceVersion::from_str("full").unwrap().has_variant());
    }

    #[test]
    fn datadir_pins_postgres_to_major_only() {
        let dirs = dirs_in(std::path::Path::new("/tmp/x"));
        let v = ServiceVersion::from_str("16.2").unwrap();
        assert_eq!(
            datadir(&dirs, "postgres", true, &v),
            PathBuf::from("/tmp/x/d/services/postgres/data-16")
        );
        let v8 = ServiceVersion::from_str("8.4").unwrap();
        assert_eq!(
            datadir(&dirs, "mysql", false, &v8),
            PathBuf::from("/tmp/x/d/services/mysql/data")
        );
    }

    /// Base and any variant of the same major resolve to one shared datadir, so a
    /// `change-version` between base and variant keeps the databases in place.
    #[test]
    fn datadir_is_shared_across_base_and_variant() {
        let dirs = dirs_in(std::path::Path::new("/tmp/x"));
        let shared = PathBuf::from("/tmp/x/d/services/postgres/data-17");
        for label in ["17", "17.10", "17.10-full", "17-full"] {
            let v = ServiceVersion::from_str(label).unwrap();
            assert_eq!(
                datadir(&dirs, "postgres", true, &v),
                shared,
                "label {label}"
            );
        }
    }

    #[test]
    fn server_path_layout() {
        let dirs = dirs_in(std::path::Path::new("/tmp/x"));
        let v = ServiceVersion::from_str("8").unwrap();
        assert_eq!(
            server_path(&dirs, "redis", "valkey-server", &v),
            PathBuf::from("/tmp/x/d/services/redis/8/bin/valkey-server")
        );
    }

    #[test]
    fn discover_finds_installed_versions_only() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let reg = ServiceRegistry::builtin();
        let v = ServiceVersion::from_str("8").unwrap();

        assert!(discover_installed(&dirs, &reg).unwrap().is_empty());

        std::fs::create_dir_all(install_dir(&dirs, "redis", &v).join("bin")).unwrap();
        assert!(discover_installed(&dirs, &reg).unwrap().is_empty());

        std::fs::write(
            server_path(&dirs, "redis", "valkey-server", &v),
            b"#!/bin/sh\n",
        )
        .unwrap();
        let found = discover_installed(&dirs, &reg).unwrap();
        assert_eq!(found.get("redis"), Some(&vec![v]));
    }
}
