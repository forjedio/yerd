//! Project-signal gathering (I/O) for web-root detection.
//!
//! This is the I/O half of framework detection: it reads a project directory on
//! disk and produces an in-memory [`ProjectSignals`]. The *decision* — turning
//! signals into a served subpath — is the pure [`yerd_core::detect::detect`].
//!
//! Per the platform crate's "side effects behind traits" rule, gathering is
//! exposed through the [`ProjectSignalSource`] trait so callers can inject a
//! fake in tests; [`FsSignalSource`] is the real filesystem-backed impl.

use std::path::Path;

use yerd_core::detect::{ProjectSignals, ROOT_MARKERS, WEB_DIR_CANDIDATES};

/// A source of [`ProjectSignals`] for a project directory.
///
/// The real impl reads the filesystem ([`FsSignalSource`]); tests can supply a
/// mock that returns canned signals without touching disk.
pub trait ProjectSignalSource {
    /// Gather signals for the project rooted at `project_root`. Best-effort:
    /// implementations must not error — missing/unreadable inputs simply yield
    /// fewer signals.
    fn gather(&self, project_root: &Path) -> ProjectSignals;
}

/// Filesystem-backed [`ProjectSignalSource`].
#[derive(Debug, Default, Clone, Copy)]
pub struct FsSignalSource;

impl ProjectSignalSource for FsSignalSource {
    fn gather(&self, project_root: &Path) -> ProjectSignals {
        gather_project_signals(project_root)
    }
}

/// Probe `project_root` on disk and build its [`ProjectSignals`].
///
/// Best-effort and infallible: a missing or malformed `composer.json`, an
/// unreadable directory, etc. just contribute no signals. Reads are limited to
/// the project root and the immediate candidate web dirs — never recursive.
#[must_use]
pub fn gather_project_signals(project_root: &Path) -> ProjectSignals {
    let mut signals = ProjectSignals::default();

    // composer.json `require` + `require-dev` package names (lowercased).
    gather_composer_requires(project_root, &mut signals);

    // Root markers — presence as a file or directory.
    for marker in ROOT_MARKERS {
        if project_root.join(marker).exists() {
            signals.markers.insert((*marker).to_string());
        }
    }

    // Candidate web dirs that contain an `index.php` front controller.
    for cand in WEB_DIR_CANDIDATES {
        if project_root.join(cand).join("index.php").is_file() {
            signals.web_dirs_with_index.insert((*cand).to_string());
        }
    }

    signals
}

/// Collect lowercased composer `require` + `require-dev` package names from
/// `project_root/composer.json` into `signals`. Best-effort: a missing or
/// malformed file contributes nothing.
fn gather_composer_requires(project_root: &Path, signals: &mut ProjectSignals) {
    let Ok(bytes) = std::fs::read(project_root.join("composer.json")) else {
        return;
    };
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return;
    };
    for section in ["require", "require-dev"] {
        let Some(map) = json.get(section).and_then(serde_json::Value::as_object) else {
            continue;
        };
        for key in map.keys() {
            signals.composer_requires.insert(key.to_ascii_lowercase());
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
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;
    use yerd_core::detect::detect;

    fn touch(path: PathBuf) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, b"").unwrap();
    }

    #[test]
    fn laravel_fixture_resolves_public() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        touch(root.join("artisan"));
        touch(root.join("public/index.php"));
        fs::write(
            root.join("composer.json"),
            br#"{"require":{"laravel/framework":"^11.0","PHP":">=8.2"}}"#,
        )
        .unwrap();

        let sig = gather_project_signals(root);
        assert!(sig.composer_requires.contains("laravel/framework"));
        assert!(sig.markers.contains("artisan"));
        assert!(sig.web_dirs_with_index.contains("public"));

        let d = detect(&sig);
        assert_eq!(d.subpath, PathBuf::from("public"));
        assert!(d.resolved);
    }

    #[test]
    fn composer_require_keys_are_lowercased() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("composer.json"),
            br#"{"require-dev":{"PHPUnit/PHPUnit":"^11"}}"#,
        )
        .unwrap();
        let sig = gather_project_signals(dir.path());
        assert!(sig.composer_requires.contains("phpunit/phpunit"));
    }

    #[test]
    fn wordpress_fixture_resolves_root() {
        let dir = TempDir::new().unwrap();
        touch(dir.path().join("wp-config.php"));
        touch(dir.path().join("index.php"));
        let d = detect(&gather_project_signals(dir.path()));
        assert_eq!(d.subpath, PathBuf::from(""));
        assert!(d.resolved);
    }

    #[test]
    fn plain_php_fixture_resolves_root() {
        let dir = TempDir::new().unwrap();
        touch(dir.path().join("index.php"));
        let d = detect(&gather_project_signals(dir.path()));
        assert_eq!(d.subpath, PathBuf::from(""));
        assert!(d.resolved);
    }

    #[test]
    fn empty_dir_is_unresolved() {
        let dir = TempDir::new().unwrap();
        let d = detect(&gather_project_signals(dir.path()));
        assert!(!d.resolved);
    }

    #[test]
    fn malformed_composer_json_is_ignored() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("composer.json"), b"{ not valid json").unwrap();
        touch(dir.path().join("public/index.php"));
        // Falls through to the generic public/ branch.
        let sig = gather_project_signals(dir.path());
        assert!(sig.composer_requires.is_empty());
        assert_eq!(detect(&sig).subpath, PathBuf::from("public"));
    }

    #[test]
    fn missing_project_dir_yields_empty_signals() {
        let sig = gather_project_signals(Path::new("/nonexistent/yerd/path/xyz"));
        assert_eq!(sig, ProjectSignals::default());
    }

    #[test]
    fn fs_source_trait_matches_free_fn() {
        let dir = TempDir::new().unwrap();
        touch(dir.path().join("public/index.php"));
        assert_eq!(
            FsSignalSource.gather(dir.path()),
            gather_project_signals(dir.path())
        );
    }
}
