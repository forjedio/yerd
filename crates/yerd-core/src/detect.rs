//! Pure web-root detection.
//!
//! Given a set of in-memory [`ProjectSignals`] (gathered by an I/O layer such as
//! `yerd-platform`), decide which subdirectory of a PHP project should be served
//! as its web root. This is the *decision* half of framework detection; it does
//! no I/O and is fully unit-testable.
//!
//! Common conventions:
//! - Laravel / Symfony (4+) / `CodeIgniter` 4 → `public/`
//! - `CakePHP` → `webroot/`
//! - Drupal (composer) / Yii2 → `web/`
//! - Magento 2 → `pub/`
//! - `WordPress`, plain PHP → project root
//!
//! The returned [`Detection::resolved`] flag tells the daemon whether detection
//! was *confident* (a framework/web-root was identified, or the project is a
//! confident root like `WordPress`) or merely a provisional root fallback for a
//! project that shows no evidence yet - the daemon keeps watching the latter.

use std::collections::BTreeSet;
use std::path::PathBuf;

/// Candidate web-root subdirectories, in generic-fallback priority order. The
/// I/O signal-gatherer probes each for an `index.php`; [`detect`] consults the
/// same list so the two never drift.
pub const WEB_DIR_CANDIDATES: &[&str] = &["public", "web", "webroot", "pub"];

/// Root marker files/dirs the signal-gatherer should probe. Presence (file *or*
/// directory) is recorded in [`ProjectSignals::markers`] under these exact keys.
pub const ROOT_MARKERS: &[&str] = &[
    "artisan",       // Laravel
    "wp-config.php", // WordPress
    "wp-load.php",   // WordPress (alt)
    "bin/console",   // Symfony
    "spark",         // CodeIgniter 4
    "bin/cake",      // CakePHP
    "bin/magento",   // Magento 2
    "yii",           // Yii2
    "index.php",     // plain PHP / legacy Drupal root
    "core",          // legacy Drupal core dir
];

/// In-memory signals about a project directory, gathered by an I/O layer.
///
/// All sets use lowercase, slash-separated keys. `composer_requires` holds the
/// package names from `composer.json`'s `require` + `require-dev`;
/// `web_dirs_with_index` holds the subset of [`WEB_DIR_CANDIDATES`] that contain
/// an `index.php`; `markers` holds the subset of [`ROOT_MARKERS`] present.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectSignals {
    /// Lowercased composer package names (`require` + `require-dev`).
    pub composer_requires: BTreeSet<String>,
    /// Present root markers (see [`ROOT_MARKERS`]).
    pub markers: BTreeSet<String>,
    /// Candidate web dirs that contain an `index.php` (subset of [`WEB_DIR_CANDIDATES`]).
    pub web_dirs_with_index: BTreeSet<String>,
}

/// The outcome of [`detect`]: the web subpath to serve (empty = project root)
/// and whether detection was confident.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection {
    /// Web root relative to the project root (`""` = serve the root).
    pub subpath: PathBuf,
    /// `true` when a framework/web-root was confidently identified (or the
    /// project is a confident root). `false` only for the provisional
    /// no-evidence root fallback - the daemon keeps watching those.
    pub resolved: bool,
}

impl Detection {
    fn sub(dir: &str) -> Self {
        Self {
            subpath: PathBuf::from(dir),
            resolved: true,
        }
    }

    fn root() -> Self {
        Self {
            subpath: PathBuf::new(),
            resolved: true,
        }
    }

    fn unresolved() -> Self {
        Self {
            subpath: PathBuf::new(),
            resolved: false,
        }
    }
}

/// A framework whose web root is a fixed subdirectory. It matches when the
/// composer `package` is required **or** the CLI `marker` is present (an empty
/// `marker` means composer-only), **and** `web_dir` actually contains an
/// `index.php`. On a match the served web root is `web_dir`.
struct FrameworkRule {
    package: &'static str,
    marker: &'static str,
    web_dir: &'static str,
}

/// `public/`-style frameworks, checked before Drupal (precedence rules 1–4).
const PUBLIC_FRAMEWORKS: &[FrameworkRule] = &[
    FrameworkRule {
        package: "laravel/framework",
        marker: "artisan",
        web_dir: "public",
    },
    FrameworkRule {
        package: "symfony/framework-bundle",
        marker: "bin/console",
        web_dir: "public",
    },
    FrameworkRule {
        package: "codeigniter4/framework",
        marker: "spark",
        web_dir: "public",
    },
    FrameworkRule {
        package: "cakephp/cakephp",
        marker: "bin/cake",
        web_dir: "webroot",
    },
];

/// Frameworks checked after Drupal (precedence rules 6–7): Yii2 (composer-only)
/// then Magento 2.
const OTHER_FRAMEWORKS: &[FrameworkRule] = &[
    FrameworkRule {
        package: "yiisoft/yii2",
        marker: "",
        web_dir: "web",
    },
    FrameworkRule {
        package: "magento/product-community-edition",
        marker: "bin/magento",
        web_dir: "pub",
    },
];

/// Decide the web root for a project from its [`ProjectSignals`]. Pure; no I/O.
///
/// Precedence is first-match-wins (see module docs).
#[must_use]
pub fn detect(sig: &ProjectSignals) -> Detection {
    if let Some(d) = match_framework(sig, PUBLIC_FRAMEWORKS) {
        return d;
    }
    if let Some(d) = detect_drupal(sig) {
        return d;
    }
    if let Some(d) = match_framework(sig, OTHER_FRAMEWORKS) {
        return d;
    }
    if sig.markers.contains("wp-config.php") || sig.markers.contains("wp-load.php") {
        return Detection::root();
    }
    for cand in WEB_DIR_CANDIDATES {
        if sig.web_dirs_with_index.contains(*cand) {
            return Detection::sub(cand);
        }
    }
    if sig.markers.contains("index.php") {
        return Detection::root();
    }
    Detection::unresolved()
}

/// First rule in `rules` (in order) that matches `sig`, mapped to its detection.
fn match_framework(sig: &ProjectSignals, rules: &[FrameworkRule]) -> Option<Detection> {
    rules.iter().find_map(|r| {
        let by_package = sig.composer_requires.contains(r.package);
        let by_marker = !r.marker.is_empty() && sig.markers.contains(r.marker);
        ((by_package || by_marker) && sig.web_dirs_with_index.contains(r.web_dir))
            .then(|| Detection::sub(r.web_dir))
    })
}

/// Rule 5: Drupal via composer `drupal/core*` - serve `web/` when present, else
/// a legacy tarball root (`index.php` + `core/`). `None` if this isn't Drupal.
fn detect_drupal(sig: &ProjectSignals) -> Option<Detection> {
    if !sig
        .composer_requires
        .iter()
        .any(|p| p.starts_with("drupal/core"))
    {
        return None;
    }
    if sig.web_dirs_with_index.contains("web") {
        return Some(Detection::sub("web"));
    }
    if sig.markers.contains("index.php") && sig.markers.contains("core") {
        return Some(Detection::root());
    }
    None
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// Build signals from slices, for terse test cases.
    fn signals(reqs: &[&str], markers: &[&str], webdirs: &[&str]) -> ProjectSignals {
        ProjectSignals {
            composer_requires: reqs.iter().map(|s| (*s).to_string()).collect(),
            markers: markers.iter().map(|s| (*s).to_string()).collect(),
            web_dirs_with_index: webdirs.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    fn sub(d: &Detection) -> &std::path::Path {
        &d.subpath
    }

    #[test]
    fn laravel_via_composer() {
        let d = detect(&signals(&["laravel/framework"], &[], &["public"]));
        assert_eq!(sub(&d), std::path::Path::new("public"));
        assert!(d.resolved);
    }

    #[test]
    fn laravel_via_artisan_marker() {
        let d = detect(&signals(&[], &["artisan"], &["public"]));
        assert_eq!(sub(&d), std::path::Path::new("public"));
        assert!(d.resolved);
    }

    #[test]
    fn symfony_modern_public() {
        let d = detect(&signals(
            &["symfony/framework-bundle"],
            &["bin/console"],
            &["public"],
        ));
        assert_eq!(sub(&d), std::path::Path::new("public"));
    }

    #[test]
    fn codeigniter4_public() {
        let d = detect(&signals(
            &["codeigniter4/framework"],
            &["spark"],
            &["public"],
        ));
        assert_eq!(sub(&d), std::path::Path::new("public"));
    }

    #[test]
    fn cakephp_webroot() {
        let d = detect(&signals(&["cakephp/cakephp"], &["bin/cake"], &["webroot"]));
        assert_eq!(sub(&d), std::path::Path::new("webroot"));
    }

    #[test]
    fn drupal_composer_web() {
        let d = detect(&signals(&["drupal/core-recommended"], &[], &["web"]));
        assert_eq!(sub(&d), std::path::Path::new("web"));
    }

    #[test]
    fn drupal_legacy_tarball_root() {
        let d = detect(&signals(&["drupal/core"], &["index.php", "core"], &[]));
        assert_eq!(sub(&d), std::path::Path::new(""));
        assert!(d.resolved);
    }

    #[test]
    fn yii2_web() {
        let d = detect(&signals(&["yiisoft/yii2"], &[], &["web"]));
        assert_eq!(sub(&d), std::path::Path::new("web"));
    }

    #[test]
    fn magento_pub() {
        let d = detect(&signals(
            &["magento/product-community-edition"],
            &["bin/magento"],
            &["pub"],
        ));
        assert_eq!(sub(&d), std::path::Path::new("pub"));
    }

    #[test]
    fn wordpress_root() {
        let d = detect(&signals(&[], &["wp-config.php", "index.php"], &[]));
        assert_eq!(sub(&d), std::path::Path::new(""));
        assert!(d.resolved);
    }

    #[test]
    fn generic_public_without_framework_markers() {
        let d = detect(&signals(&[], &[], &["public"]));
        assert_eq!(sub(&d), std::path::Path::new("public"));
        assert!(d.resolved);
    }

    #[test]
    fn generic_prefers_public_over_web() {
        let d = detect(&signals(&[], &[], &["web", "public"]));
        assert_eq!(sub(&d), std::path::Path::new("public"));
    }

    #[test]
    fn plain_php_root_index() {
        let d = detect(&signals(&[], &["index.php"], &[]));
        assert_eq!(sub(&d), std::path::Path::new(""));
        assert!(d.resolved);
    }

    #[test]
    fn empty_project_is_unresolved_root() {
        let d = detect(&signals(&[], &[], &[]));
        assert_eq!(sub(&d), std::path::Path::new(""));
        assert!(!d.resolved);
    }

    #[test]
    fn laravel_marker_without_public_yet_is_unresolved() {
        let d = detect(&signals(&[], &["artisan"], &[]));
        assert_eq!(sub(&d), std::path::Path::new(""));
        assert!(!d.resolved);
    }

    #[test]
    fn laravel_precedence_beats_generic_web() {
        let d = detect(&signals(
            &["laravel/framework"],
            &["artisan"],
            &["public", "web"],
        ));
        assert_eq!(sub(&d), std::path::Path::new("public"));
    }
}
