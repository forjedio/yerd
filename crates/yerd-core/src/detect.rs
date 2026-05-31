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
//! project that shows no evidence yet — the daemon keeps watching the latter.

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
    /// no-evidence root fallback — the daemon keeps watching those.
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

/// Decide the web root for a project from its [`ProjectSignals`]. Pure; no I/O.
///
/// Precedence is first-match-wins (see module docs and the inline comments).
#[must_use]
pub fn detect(sig: &ProjectSignals) -> Detection {
    let req = |pkg: &str| sig.composer_requires.contains(pkg);
    let req_prefix = |prefix: &str| sig.composer_requires.iter().any(|p| p.starts_with(prefix));
    let marker = |m: &str| sig.markers.contains(m);
    let webdir = |d: &str| sig.web_dirs_with_index.contains(d);

    // 1. Laravel — composer or `artisan`, serving `public/`.
    if (req("laravel/framework") || marker("artisan")) && webdir("public") {
        return Detection::sub("public");
    }
    // 2. Symfony (modern 4+) — composer or `bin/console`, serving `public/`.
    if (req("symfony/framework-bundle") || marker("bin/console")) && webdir("public") {
        return Detection::sub("public");
    }
    // 3. CodeIgniter 4 — composer or `spark`, serving `public/`.
    if (req("codeigniter4/framework") || marker("spark")) && webdir("public") {
        return Detection::sub("public");
    }
    // 4. CakePHP — composer or `bin/cake`, serving `webroot/`.
    if (req("cakephp/cakephp") || marker("bin/cake")) && webdir("webroot") {
        return Detection::sub("webroot");
    }
    // 5. Drupal (composer `drupal/core*`) — `web/`, else legacy tarball root.
    if req_prefix("drupal/core") {
        if webdir("web") {
            return Detection::sub("web");
        }
        if marker("index.php") && marker("core") {
            return Detection::root();
        }
    }
    // 6. Yii2 — composer, serving `web/`.
    if req("yiisoft/yii2") && webdir("web") {
        return Detection::sub("web");
    }
    // 7. Magento 2 — composer or `bin/magento`, serving `pub/`.
    if (req("magento/product-community-edition") || marker("bin/magento")) && webdir("pub") {
        return Detection::sub("pub");
    }
    // 8. WordPress — root-served.
    if marker("wp-config.php") || marker("wp-load.php") {
        return Detection::root();
    }
    // 9. Generic — first candidate web dir that has an index.php.
    for cand in WEB_DIR_CANDIDATES {
        if webdir(cand) {
            return Detection::sub(cand);
        }
    }
    // 10. Plain PHP — index.php at the root.
    if marker("index.php") {
        return Detection::root();
    }
    // 11. No evidence — serve root provisionally, keep watching.
    Detection::unresolved()
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
        // `artisan` present but `public/index.php` not cloned yet → keep watching.
        let d = detect(&signals(&[], &["artisan"], &[]));
        assert_eq!(sub(&d), std::path::Path::new(""));
        assert!(!d.resolved);
    }

    #[test]
    fn laravel_precedence_beats_generic_web() {
        // Both a Laravel public/ and a stray web/ with index.php → Laravel wins.
        let d = detect(&signals(
            &["laravel/framework"],
            &["artisan"],
            &["public", "web"],
        ));
        assert_eq!(sub(&d), std::path::Path::new("public"));
    }
}
