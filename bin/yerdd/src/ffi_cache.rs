//! FFI-capability cache for installed PHP versions.
//!
//! The `PhpVersions` reply lists which installed versions include the FFI
//! extension, so the GUI only offers its FFI toggle where it can work. Finding
//! out means spawning `php -n -m` per version ([`yerd_php::probe_ffi`]), and
//! that reply goes out for every `ListPhp` and every PHP-settings change, so
//! results are cached per version on a freshness stamp of the CLI binary
//! (`mtime`, size). Installing or updating a version replaces the binary, which
//! changes the stamp and triggers a fresh probe. Same idea as
//! [`crate::detect_cache`].
//!
//! The CLI binary stands in for `php-fpm`: both come from the same yerd-php
//! build, so they share the same compiled-in extensions. Legacy versions are
//! never probed, since `ffi.enable` is never rendered into their pools
//! ([`yerd_core::php_settings::applies_to_version`]); uncached versions are
//! probed concurrently.
//!
//! The same answers gate pool rendering: [`FfiCache::gate_ini_overrides`] adds
//! an unpersisted `ffi.enable = Off` override for a version whose build
//! definitively lacks FFI (a stable build that predates FFI support), so FPM
//! is never handed a `php_admin_value` for a directive it does not know. An
//! unanswered probe leaves the setting alone.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::SystemTime;

use yerd_core::PhpVersion;
use yerd_php::CommandRunner;

/// A CLI binary's `(modified time, size)`.
type Stamp = (SystemTime, u64);

/// Caches whether each installed PHP version's build includes FFI.
pub struct FfiCache {
    runner: Arc<dyn CommandRunner>,
    inner: Mutex<HashMap<PhpVersion, (Stamp, bool)>>,
}

impl FfiCache {
    /// A fresh, empty cache that probes through `runner`.
    #[must_use]
    pub fn new(runner: Arc<dyn CommandRunner>) -> Self {
        Self {
            runner,
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// The versions in `bins` (version plus its CLI binary path) whose build
    /// includes FFI, in input order. Legacy versions and missing binaries count
    /// as not capable and are not probed; a probe that cannot answer counts as
    /// not capable and is not cached, so the next call retries it.
    pub async fn capable_versions(&self, bins: &[(PhpVersion, PathBuf)]) -> Vec<PhpVersion> {
        let known = self.answers(bins).await;
        bins.iter()
            .map(|(v, _)| *v)
            .filter(|v| known.get(v).copied().unwrap_or(false))
            .collect()
    }

    /// The per-version ini overrides to hand the `PhpManager`: `overrides`
    /// plus `ffi.enable = Off` for every version in `bins` whose effective
    /// `ffi.enable` is on but whose build definitively lacks FFI. Only those
    /// versions are probed, so nothing is spawned while FFI is off everywhere.
    /// The added entries are never persisted to the config.
    pub async fn gate_ini_overrides(
        &self,
        global: &BTreeMap<String, String>,
        overrides: &BTreeMap<PhpVersion, BTreeMap<String, String>>,
        bins: &[(PhpVersion, PathBuf)],
    ) -> BTreeMap<PhpVersion, BTreeMap<String, String>> {
        let requested: Vec<(PhpVersion, PathBuf)> = bins
            .iter()
            .filter(|(v, _)| ffi_requested(global, overrides.get(v)))
            .cloned()
            .collect();
        let known = self.answers(&requested).await;
        let lacking: Vec<PhpVersion> = requested
            .iter()
            .map(|(v, _)| *v)
            .filter(|v| known.get(v) == Some(&false))
            .collect();
        gated_overrides(overrides, &lacking)
    }

    /// Definitive FFI answers for the applicable versions in `bins`, from the
    /// cache or from concurrent probes. Unanswered versions are absent.
    async fn answers(&self, bins: &[(PhpVersion, PathBuf)]) -> HashMap<PhpVersion, bool> {
        let mut known: HashMap<PhpVersion, bool> = HashMap::new();
        let mut probes = tokio::task::JoinSet::new();
        for (version, bin) in bins {
            if !yerd_core::php_settings::applies_to_version("ffi.enable", *version) {
                continue;
            }
            let Some(stamp) = stamp_of(bin) else {
                continue;
            };
            let cached = self
                .lock()
                .get(version)
                .and_then(|(s, capable)| (*s == stamp).then_some(*capable));
            if let Some(capable) = cached {
                known.insert(*version, capable);
                continue;
            }
            let runner = Arc::clone(&self.runner);
            let (version, bin) = (*version, bin.clone());
            probes.spawn(async move {
                let answer = yerd_php::probe_ffi(runner.as_ref(), &bin).await;
                (version, stamp, answer)
            });
        }
        while let Some(joined) = probes.join_next().await {
            let Ok((version, stamp, Some(capable))) = joined else {
                continue;
            };
            self.lock().insert(version, (stamp, capable));
            known.insert(version, capable);
        }
        known
    }

    /// Lock helper that recovers from a poisoned mutex rather than panicking;
    /// the worst case is a stale-but-valid capability answer.
    fn lock(&self) -> MutexGuard<'_, HashMap<PhpVersion, (Stamp, bool)>> {
        match self.inner.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

/// Whether a version's effective `ffi.enable` (its override, else the global
/// value) would render a pool line.
fn ffi_requested(
    global: &BTreeMap<String, String>,
    overrides: Option<&BTreeMap<String, String>>,
) -> bool {
    let empty = BTreeMap::new();
    yerd_core::php_settings::merge_effective(global, overrides.unwrap_or(&empty))
        .get("ffi.enable")
        .is_some_and(|v| yerd_core::php_settings::fpm_value("ffi.enable", v).is_some())
}

/// `overrides` with `ffi.enable = Off` forced for every version in `lacking`.
fn gated_overrides(
    overrides: &BTreeMap<PhpVersion, BTreeMap<String, String>>,
    lacking: &[PhpVersion],
) -> BTreeMap<PhpVersion, BTreeMap<String, String>> {
    let mut out = overrides.clone();
    for v in lacking {
        out.entry(*v)
            .or_default()
            .insert("ffi.enable".to_owned(), "Off".to_owned());
    }
    out
}

fn stamp_of(bin: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(bin).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::io;
    use std::process::Command;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use yerd_php::ProbeOutput;

    use super::*;

    struct CountingRunner {
        calls: AtomicUsize,
        fail: bool,
        ffi: bool,
    }

    impl CountingRunner {
        fn new(fail: bool) -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                fail,
                ffi: true,
            })
        }

        fn without_ffi() -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                fail: false,
                ffi: false,
            })
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl CommandRunner for CountingRunner {
        async fn run(&self, _cmd: Command) -> Result<ProbeOutput, io::Error> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                return Err(io::Error::from(io::ErrorKind::NotFound));
            }
            let stdout = if self.ffi {
                "[PHP Modules]\nCore\nFFI\n"
            } else {
                "[PHP Modules]\nCore\n"
            };
            Ok(ProbeOutput {
                status_ok: true,
                stdout: stdout.to_owned(),
                stderr: String::new(),
            })
        }
    }

    #[tokio::test]
    async fn caches_until_the_binary_changes() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("php");
        std::fs::write(&bin, b"one").unwrap();
        let runner = CountingRunner::new(false);
        let cache = FfiCache::new(runner.clone());
        let v = PhpVersion::new(8, 4);
        let bins = vec![(v, bin.clone())];

        assert_eq!(cache.capable_versions(&bins).await, vec![v]);
        assert_eq!(runner.calls(), 1);
        assert_eq!(cache.capable_versions(&bins).await, vec![v]);
        assert_eq!(runner.calls(), 1);

        std::fs::write(&bin, b"a longer binary").unwrap();
        assert_eq!(cache.capable_versions(&bins).await, vec![v]);
        assert_eq!(runner.calls(), 2);
    }

    #[tokio::test]
    async fn legacy_versions_are_never_probed_or_reported() {
        let dir = tempfile::tempdir().unwrap();
        let legacy_bin = dir.path().join("php74");
        let modern_bin = dir.path().join("php84");
        std::fs::write(&legacy_bin, b"x").unwrap();
        std::fs::write(&modern_bin, b"x").unwrap();
        let runner = CountingRunner::new(false);
        let cache = FfiCache::new(runner.clone());
        let legacy = PhpVersion::new(7, 4);
        let modern = PhpVersion::new(8, 4);
        let bins = vec![(legacy, legacy_bin), (modern, modern_bin)];
        assert_eq!(cache.capable_versions(&bins).await, vec![modern]);
        assert_eq!(runner.calls(), 1);
    }

    #[tokio::test]
    async fn uncached_versions_are_all_probed_and_reported_in_input_order() {
        let dir = tempfile::tempdir().unwrap();
        let versions = [
            PhpVersion::new(8, 5),
            PhpVersion::new(8, 2),
            PhpVersion::new(8, 4),
        ];
        let bins: Vec<(PhpVersion, PathBuf)> = versions
            .iter()
            .map(|v| {
                let bin = dir.path().join(format!("php{v}"));
                std::fs::write(&bin, b"x").unwrap();
                (*v, bin)
            })
            .collect();
        let runner = CountingRunner::new(false);
        let cache = FfiCache::new(runner.clone());
        assert_eq!(cache.capable_versions(&bins).await, versions.to_vec());
        assert_eq!(runner.calls(), 3);
        assert_eq!(cache.capable_versions(&bins).await, versions.to_vec());
        assert_eq!(runner.calls(), 3);
    }

    #[tokio::test]
    async fn missing_binary_is_not_capable_and_not_probed() {
        let dir = tempfile::tempdir().unwrap();
        let runner = CountingRunner::new(false);
        let cache = FfiCache::new(runner.clone());
        let bins = vec![(PhpVersion::new(8, 4), dir.path().join("absent"))];
        assert!(cache.capable_versions(&bins).await.is_empty());
        assert_eq!(runner.calls(), 0);
    }

    #[tokio::test]
    async fn failed_probe_is_not_cached() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("php");
        std::fs::write(&bin, b"x").unwrap();
        let runner = CountingRunner::new(true);
        let cache = FfiCache::new(runner.clone());
        let bins = vec![(PhpVersion::new(8, 4), bin)];
        assert!(cache.capable_versions(&bins).await.is_empty());
        assert!(cache.capable_versions(&bins).await.is_empty());
        assert_eq!(runner.calls(), 2);
    }

    fn on(name: &str) -> BTreeMap<String, String> {
        BTreeMap::from([(name.to_owned(), "On".to_owned())])
    }

    fn one_bin(dir: &Path, v: PhpVersion) -> Vec<(PhpVersion, PathBuf)> {
        let bin = dir.join(format!("php{v}"));
        std::fs::write(&bin, b"x").unwrap();
        vec![(v, bin)]
    }

    #[tokio::test]
    async fn gate_forces_off_for_a_build_without_ffi() {
        let dir = tempfile::tempdir().unwrap();
        let v = PhpVersion::new(8, 4);
        let cache = FfiCache::new(CountingRunner::without_ffi());
        let gated = cache
            .gate_ini_overrides(&on("ffi.enable"), &BTreeMap::new(), &one_bin(dir.path(), v))
            .await;
        assert_eq!(
            gated
                .get(&v)
                .and_then(|m| m.get("ffi.enable"))
                .map(String::as_str),
            Some("Off")
        );
    }

    #[tokio::test]
    async fn gate_leaves_capable_and_unanswered_versions_alone() {
        let dir = tempfile::tempdir().unwrap();
        let v = PhpVersion::new(8, 4);
        let existing = BTreeMap::from([(v, on("display_errors"))]);
        for runner in [CountingRunner::new(false), CountingRunner::new(true)] {
            let cache = FfiCache::new(runner);
            let gated = cache
                .gate_ini_overrides(&on("ffi.enable"), &existing, &one_bin(dir.path(), v))
                .await;
            assert_eq!(gated, existing);
        }
    }

    #[tokio::test]
    async fn gate_probes_nothing_while_ffi_is_off() {
        let dir = tempfile::tempdir().unwrap();
        let v = PhpVersion::new(8, 4);
        let runner = CountingRunner::without_ffi();
        let cache = FfiCache::new(runner.clone());
        let per_version_off = BTreeMap::from([(
            v,
            BTreeMap::from([("ffi.enable".to_owned(), "Off".to_owned())]),
        )]);
        let gated = cache
            .gate_ini_overrides(&on("ffi.enable"), &per_version_off, &one_bin(dir.path(), v))
            .await;
        assert_eq!(gated, per_version_off);
        let gated = cache
            .gate_ini_overrides(&BTreeMap::new(), &BTreeMap::new(), &one_bin(dir.path(), v))
            .await;
        assert!(gated.is_empty());
        assert_eq!(runner.calls(), 0);
    }
}
