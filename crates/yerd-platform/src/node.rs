//! Filesystem discovery for managed Node releases, including the legacy layout.
use crate::PlatformDirs;
use std::path::{Path, PathBuf};

/// A usable managed release.
#[derive(Debug, Clone)]
pub struct InstalledNode {
    /// Exact numeric version.
    pub version: String,
    /// Directory containing node, npm and npx.
    pub bin: PathBuf,
    /// LTS status recorded when downloaded.
    pub lts: bool,
}

/// Root for side-by-side Node releases.
#[must_use]
pub fn versions_dir(dirs: &PlatformDirs) -> PathBuf {
    dirs.data.join("tools/node-versions")
}

fn discover(root: &Path) -> Option<InstalledNode> {
    let version = std::fs::read_to_string(root.join(".version")).ok()?;
    let version = version
        .trim()
        .strip_prefix('v')
        .unwrap_or(version.trim())
        .to_owned();
    if yerd_core::node::numeric_selector(&version).ok()?.len() != 3 {
        return None;
    }
    let bin = std::fs::read_dir(root)
        .ok()?
        .flatten()
        .map(|entry| entry.path().join("bin"))
        .find(|bin| {
            bin.join("node").is_file() && bin.join("npm").is_file() && bin.join("npx").is_file()
        })?;
    Some(InstalledNode {
        version,
        bin,
        lts: root.join(".lts").is_file(),
    })
}

/// Discover complete releases without invoking binaries or downloading anything.
#[must_use]
pub fn installed(dirs: &PlatformDirs) -> Vec<InstalledNode> {
    let mut versions: Vec<_> = std::fs::read_dir(versions_dir(dirs))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        .filter_map(|entry| discover(&entry.path()))
        .collect();
    if let Some(legacy) = discover(&dirs.data.join("tools/node")) {
        if !versions.iter().any(|v| v.version == legacy.version) {
            versions.push(legacy);
        }
    }
    versions.sort_by_key(|v| yerd_core::node::numeric_selector(&v.version).unwrap_or_default());
    versions
}

/// Resolve a selector using only installed, complete releases.
///
/// # Errors
/// Invalid selectors and unavailable releases are errors with an install hint.
pub fn resolve(dirs: &PlatformDirs, selector: &str) -> Result<InstalledNode, String> {
    let versions = installed(dirs);
    let exact = yerd_core::node::select_release(
        selector,
        versions.iter().map(|v| (v.version.as_str(), v.lts)),
    )?;
    versions
        .into_iter()
        .find(|v| v.version == exact)
        .ok_or_else(|| "selected Node release disappeared".to_owned())
}

/// Default recorded by the latest-LTS installer, or the original single install.
#[must_use]
pub fn legacy_default(dirs: &PlatformDirs) -> Option<String> {
    let root = dirs.data.join("tools/node");
    std::fs::read_to_string(root.join(".default"))
        .or_else(|_| std::fs::read_to_string(root.join(".version")))
        .ok()
        .map(|v| v.trim().trim_start_matches('v').to_owned())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn staged_and_backup_releases_are_never_selected() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = PlatformDirs {
            config: tmp.path().join("c"),
            data: tmp.path().join("d"),
            state: tmp.path().join("s"),
            cache: tmp.path().join("ca"),
            runtime: tmp.path().join("r"),
        };
        for name in ["24.1.0", ".staging-24.9.0-123-0", ".previous-24.9.0-123-0"] {
            let root = versions_dir(&dirs).join(name);
            let bin = root.join("node-test/bin");
            std::fs::create_dir_all(&bin).unwrap();
            std::fs::write(
                root.join(".version"),
                if name.starts_with('.') {
                    "24.9.0"
                } else {
                    "24.1.0"
                },
            )
            .unwrap();
            for tool in ["node", "npm", "npx"] {
                std::fs::write(bin.join(tool), "fake").unwrap();
            }
        }
        assert_eq!(installed(&dirs).len(), 1);
        assert_eq!(resolve(&dirs, "24").unwrap().version, "24.1.0");
    }
}
