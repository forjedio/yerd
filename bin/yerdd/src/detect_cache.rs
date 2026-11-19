//! Project-signal cache for web-root and framework detection.
//!
//! Detection reads `composer.json` and stats a handful of files per project.
//! `scan_sites` and the per-site framework cache run on every config mutation
//! *and* every filesystem-watcher tick, so without a cache an unrelated
//! `yerd use`/`yerd secure` would re-read every project. This cache keys the
//! gathered [`ProjectSignals`] on a cheap freshness
//! stamp - `max(project-root dir mtime, composer.json mtime)` - so a rescan that
//! finds nothing changed reuses the previous result after one or two `stat`s.
//!
//! The stamp captures the two ways a project's web root can change: adding or
//! removing a top-level entry (e.g. cloning in `public/`) bumps the directory
//! mtime; editing `composer.json` in place bumps that file's mtime.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use yerd_core::{Detection, ProjectSignals};
use yerd_platform::gather_project_signals;

/// Caches per-project [`ProjectSignals`], keyed on a freshness stamp.
///
/// Uses a `std::sync::Mutex` (not tokio) because the critical section is a brief
/// map lookup/insert with no `.await`; `scan_sites` is synchronous.
#[derive(Default)]
pub struct DetectCache {
    inner: Mutex<HashMap<PathBuf, (SystemTime, ProjectSignals)>>,
}

impl DetectCache {
    /// A fresh, empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Detect the web root for `project_root` from its cached signals.
    pub fn detect(&self, project_root: &Path) -> Detection {
        yerd_core::detect(&self.signals(project_root))
    }

    /// The project's signals, reusing a cached copy while its freshness stamp
    /// is unchanged. When the stamp can't be read (unreadable dir), the signals
    /// are gathered fresh and not cached.
    pub fn signals(&self, project_root: &Path) -> ProjectSignals {
        let Some(stamp) = freshness_stamp(project_root) else {
            return gather_project_signals(project_root);
        };
        let mut guard = self.lock();
        if let Some((cached_stamp, sig)) = guard.get(project_root) {
            if *cached_stamp == stamp {
                return sig.clone();
            }
        }
        let sig = gather_project_signals(project_root);
        guard.insert(project_root.to_path_buf(), (stamp, sig.clone()));
        sig
    }

    /// Lock helper that recovers from a poisoned mutex rather than panicking
    /// (the crate forbids `unwrap`/`expect`). A poisoned detection cache is
    /// harmless - the worst case is stale-but-valid signals.
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<PathBuf, (SystemTime, ProjectSignals)>> {
        match self.inner.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

/// `max(project-root dir mtime, composer.json mtime)`, or `None` if neither is
/// stat-able (forces a fresh, uncached detection).
fn freshness_stamp(root: &Path) -> Option<SystemTime> {
    let dir = std::fs::metadata(root).and_then(|m| m.modified()).ok();
    let composer = std::fs::metadata(root.join("composer.json"))
        .and_then(|m| m.modified())
        .ok();
    match (dir, composer) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (Some(a), None) | (None, Some(a)) => Some(a),
        (None, None) => None,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn detects_and_caches() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("public")).unwrap();
        fs::write(dir.path().join("public/index.php"), b"").unwrap();
        let cache = DetectCache::new();
        let first = cache.detect(dir.path());
        assert_eq!(first.subpath, PathBuf::from("public"));
        let second = cache.detect(dir.path());
        assert_eq!(first, second);
    }

    #[test]
    fn caches_signals_until_composer_changes() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("composer.json"),
            br#"{"require":{"statamic/cms":"^5"}}"#,
        )
        .unwrap();
        let cache = DetectCache::new();
        let first = cache.signals(dir.path());
        assert!(first.composer_requires.contains("statamic/cms"));
        assert_eq!(cache.lock().len(), 1);
        assert_eq!(cache.signals(dir.path()), first);
        assert_eq!(
            yerd_core::detect_framework(&first),
            Some(yerd_core::SiteFramework::Statamic)
        );
    }

    #[test]
    fn unreadable_root_returns_unresolved_without_caching() {
        let cache = DetectCache::new();
        let det = cache.detect(Path::new("/nonexistent/yerd/xyz"));
        assert!(!det.resolved);
        assert!(cache.lock().is_empty());
    }
}
