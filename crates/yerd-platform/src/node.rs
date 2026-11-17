//! Filesystem discovery for managed Node releases, including the legacy layout,
//! and the project-file walk shared by the CLI and the daemon.
use crate::PlatformDirs;
use std::path::{Path, PathBuf};
use yerd_core::node::{NodeSource, ProjectFileKind, SEARCH_ORDER};

/// A usable managed release.
#[derive(Debug, Clone)]
pub struct InstalledNode {
    /// Exact numeric version.
    pub version: String,
    /// Directory containing node, npm and npx.
    pub bin: PathBuf,
    /// LTS status recorded when downloaded.
    pub lts: bool,
    /// Release directory (the side-by-side version dir or the legacy root).
    pub root: PathBuf,
    /// LTS codename recorded in `.lts`; `None` for an empty marker.
    pub codename: Option<String>,
}

impl InstalledNode {
    /// This release as a selector candidate, codename included, so
    /// `lts/<codename>` resolves against it.
    #[must_use]
    pub fn candidate(&self) -> yerd_core::node::Candidate<'_> {
        yerd_core::node::Candidate {
            version: &self.version,
            lts: self.lts,
            codename: self.codename.as_deref(),
        }
    }
}

/// Root for side-by-side Node releases.
#[must_use]
pub fn versions_dir(dirs: &PlatformDirs) -> PathBuf {
    dirs.data.join("tools/node-versions")
}

/// `legacy` marks the pre-side-by-side `{data}/tools/node` root, whose installer
/// only ever fetched the latest LTS and wrote no `.lts` marker.
fn discover(root: &Path, legacy: bool) -> Option<InstalledNode> {
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
    let lts_file = root.join(".lts");
    let codename = std::fs::read_to_string(&lts_file)
        .ok()
        .map(|c| c.trim().to_owned())
        .filter(|c| !c.is_empty());
    Some(InstalledNode {
        version,
        bin,
        lts: legacy || lts_file.is_file(),
        root: root.to_path_buf(),
        codename,
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
        .filter_map(|entry| discover(&entry.path(), false))
        .collect();
    if let Some(legacy) = discover(&dirs.data.join("tools/node"), true) {
        if !versions.iter().any(|v| v.version == legacy.version) {
            versions.push(legacy);
        }
    }
    versions.sort_by_key(|v| yerd_core::node::numeric_selector(&v.version).unwrap_or_default());
    versions
}

/// The global default Node selector: `configured` (the `[node] default` from
/// config), else the legacy single-version install's version. The daemon (when
/// linking and reporting) and the CLI (when a shim or `yerd exec` runs Node) both
/// use this, so they always agree on which release is the default.
#[must_use]
pub fn default_selector(dirs: &PlatformDirs, configured: Option<&str>) -> Option<String> {
    configured
        .map(str::to_owned)
        .or_else(|| legacy_default(dirs))
}

/// Resolve a selector using only installed, complete releases.
///
/// # Errors
/// Invalid selectors and unavailable releases are errors with an install hint.
pub fn resolve(dirs: &PlatformDirs, selector: &str) -> Result<InstalledNode, String> {
    let versions = installed(dirs);
    let exact =
        yerd_core::node::select_release(selector, versions.iter().map(InstalledNode::candidate))?;
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

/// A project's Node version file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectFile {
    /// Which file it is.
    pub kind: ProjectFileKind,
    /// The validated selector it contains.
    pub selector: String,
    /// Where it was found.
    pub path: PathBuf,
}

/// Find the nearest `.nvmrc` or `.node-version`, walking up from `start` and
/// stopping at `root`. Within one directory `.nvmrc` wins.
///
/// # Errors
/// An existing file that cannot be read, is empty or is unsupported must fail.
pub fn project_file(start: &Path, root: Option<&Path>) -> Result<Option<ProjectFile>, String> {
    for dir in start.ancestors() {
        for kind in SEARCH_ORDER {
            let file = dir.join(kind.file_name());
            match std::fs::symlink_metadata(&file) {
                Ok(_) => {
                    let value = std::fs::read_to_string(&file)
                        .map_err(|e| format!("cannot read {}: {e}", file.display()))?;
                    let selector = yerd_core::node::parse_project_selector(&value)
                        .map_err(|e| format!("invalid {}: {e}", file.display()))?;
                    return Ok(Some(ProjectFile {
                        kind,
                        selector,
                        path: file,
                    }));
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("cannot inspect {}: {e}", file.display())),
            }
        }
        if root == Some(dir) {
            break;
        }
    }
    Ok(None)
}

/// A resolved release and where its selector came from.
#[derive(Debug, Clone)]
pub struct Selected {
    /// The installed release.
    pub installed: InstalledNode,
    /// Which source chose it.
    pub source: NodeSource,
}

/// Select a release: project file, then site preference, then global default.
///
/// # Errors
/// Invalid project files, a missing default, and uninstalled versions are errors.
pub fn select(
    dirs: &PlatformDirs,
    start: &Path,
    root: Option<&Path>,
    preference: Option<&str>,
    default: Option<&str>,
) -> Result<Selected, String> {
    let file = project_file(start, root)?;
    select_from(dirs, file.as_ref(), preference, default)
}

/// [`select`] with the project file already found, so a caller that reports
/// the file too walks the directory tree once.
///
/// # Errors
/// A missing default and uninstalled versions are errors.
pub fn select_from(
    dirs: &PlatformDirs,
    file: Option<&ProjectFile>,
    preference: Option<&str>,
    default: Option<&str>,
) -> Result<Selected, String> {
    let (selector, source) = yerd_core::node::choose(
        file.map(|f| (f.selector.as_str(), f.kind)),
        preference,
        default,
    )
    .ok_or_else(|| {
        "no global Node default — run `yerd install tool node` or `yerd node use <version>`"
            .to_owned()
    })?;
    Ok(Selected {
        installed: resolve(dirs, selector)?,
        source,
    })
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

    fn temp_dirs(tmp: &Path) -> PlatformDirs {
        PlatformDirs {
            config: tmp.join("c"),
            data: tmp.join("d"),
            state: tmp.join("s"),
            cache: tmp.join("ca"),
            runtime: tmp.join("r"),
        }
    }

    fn fake_release(root: &Path, version: &str) {
        let bin = root.join(format!("node-v{version}-test/bin"));
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(root.join(".version"), version).unwrap();
        for tool in ["node", "npm", "npx"] {
            std::fs::write(bin.join(tool), "fake").unwrap();
        }
    }

    #[test]
    fn project_file_precedence_and_boundaries() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        let nested = project.join("src/deep");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(tmp.path().join(".nvmrc"), "18").unwrap();
        assert_eq!(project_file(&nested, Some(&project)).unwrap(), None);
        std::fs::write(project.join(".nvmrc"), "22").unwrap();
        std::fs::write(project.join(".node-version"), "20\n").unwrap();
        let found = project_file(&nested, Some(&project)).unwrap().unwrap();
        assert_eq!(
            (found.kind, found.selector.as_str(), found.path),
            (ProjectFileKind::Nvmrc, "22", project.join(".nvmrc"))
        );
        std::fs::write(project.join("src/.node-version"), "v24.1").unwrap();
        let found = project_file(&nested, Some(&project)).unwrap().unwrap();
        assert_eq!(
            (found.kind, found.selector.as_str()),
            (ProjectFileKind::NodeVersion, "v24.1")
        );
        std::fs::remove_file(project.join("src/.node-version")).unwrap();
        std::fs::create_dir(project.join("src/.node-version")).unwrap();
        assert!(project_file(&nested, Some(&project)).is_err());
        std::fs::remove_dir(project.join("src/.node-version")).unwrap();
        std::fs::write(project.join("src/.node-version"), "").unwrap();
        assert!(project_file(&nested, Some(&project)).is_err());
    }

    #[test]
    fn installed_records_root_and_codename() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = temp_dirs(tmp.path());
        for (version, lts) in [
            ("22.9.0", Some("Jod\n")),
            ("24.1.0", Some("")),
            ("25.0.0", None),
        ] {
            let root = versions_dir(&dirs).join(version);
            fake_release(&root, version);
            if let Some(contents) = lts {
                std::fs::write(root.join(".lts"), contents).unwrap();
            }
        }
        let found: Vec<_> = installed(&dirs)
            .into_iter()
            .map(|n| (n.version, n.lts, n.codename, n.root))
            .collect();
        assert_eq!(
            found,
            vec![
                (
                    "22.9.0".to_owned(),
                    true,
                    Some("Jod".to_owned()),
                    versions_dir(&dirs).join("22.9.0")
                ),
                (
                    "24.1.0".to_owned(),
                    true,
                    None,
                    versions_dir(&dirs).join("24.1.0")
                ),
                (
                    "25.0.0".to_owned(),
                    false,
                    None,
                    versions_dir(&dirs).join("25.0.0")
                ),
            ]
        );
    }

    #[test]
    fn select_follows_file_then_preference_then_default() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = temp_dirs(tmp.path());
        for version in ["20.1.0", "22.9.0", "24.1.0"] {
            fake_release(&versions_dir(&dirs).join(version), version);
        }
        let project = tmp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let pick = |preference, default| {
            select(&dirs, &project, Some(&project), preference, default)
                .map(|s| (s.installed.version, s.source))
        };
        assert!(pick(None, None)
            .unwrap_err()
            .contains("no global Node default"));
        assert_eq!(
            pick(None, Some("20")).unwrap(),
            ("20.1.0".to_owned(), NodeSource::Default)
        );
        assert_eq!(
            pick(Some("24"), Some("20")).unwrap(),
            ("24.1.0".to_owned(), NodeSource::Site)
        );
        std::fs::write(project.join(".node-version"), "22").unwrap();
        assert_eq!(
            pick(Some("24"), Some("20")).unwrap(),
            ("22.9.0".to_owned(), NodeSource::NodeVersion)
        );
        std::fs::write(project.join(".nvmrc"), "18").unwrap();
        assert!(pick(Some("24"), Some("20"))
            .unwrap_err()
            .contains("yerd install tool node 18"));
    }

    #[test]
    fn codename_selectors_resolve_against_recorded_lts_markers() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = temp_dirs(tmp.path());
        for (version, marker) in [
            ("20.18.0", "Iron"),
            ("20.19.0", "Iron\n"),
            ("22.12.0", "Jod"),
        ] {
            let root = versions_dir(&dirs).join(version);
            fake_release(&root, version);
            std::fs::write(root.join(".lts"), marker).unwrap();
        }
        let project = tmp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join(".nvmrc"), "lts/iron\n").unwrap();
        let selected = select(&dirs, &project, Some(&project), None, Some("22")).unwrap();
        assert_eq!(
            (selected.installed.version.as_str(), selected.source),
            ("20.19.0", NodeSource::Nvmrc)
        );
        assert_eq!(resolve(&dirs, "lts/JOD").unwrap().version, "22.12.0");
        assert!(resolve(&dirs, "lts/hydrogen")
            .unwrap_err()
            .contains("yerd install tool node lts/hydrogen"));
    }
}
