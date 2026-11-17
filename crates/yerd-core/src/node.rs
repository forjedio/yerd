//! Numeric Node release selectors and the pure Node version decisions shared
//! by installation, execution, and the daemon's Node handlers.

use std::collections::BTreeMap;

/// Parse a numeric selector with one to three components and optional `v`.
///
/// # Errors
/// Rejects aliases, malformed versions, and values outside `u32`.
pub fn numeric_selector(value: &str) -> Result<Vec<u32>, String> {
    let value = value.strip_prefix('v').unwrap_or(value);
    let parts: Vec<_> = value.split('.').collect();
    if parts.is_empty() || parts.len() > 3 {
        return Err("expected a Node version such as 24, 24.1, or 24.1.0".to_owned());
    }
    parts
        .into_iter()
        .map(|p| {
            if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
                return Err(
                    "expected a numeric Node version such as 24, 24.1, or 24.1.0".to_owned(),
                );
            }
            p.parse::<u32>()
                .map_err(|_| "Node version component is too large".to_owned())
        })
        .collect()
}

/// Select the highest matching exact release, independently of input order.
/// `node` selects the newest installed release. `lts/*` selects the newest
/// release whose accompanying LTS flag is true.
///
/// # Errors
/// Rejects unsupported aliases or returns an install hint when no release matches.
pub fn select_release<'a>(
    selector: &str,
    releases: impl IntoIterator<Item = (&'a str, bool)>,
) -> Result<String, String> {
    let numeric = match selector {
        "node" | "lts/*" => None,
        _ => Some(numeric_selector(selector)?),
    };
    releases
        .into_iter()
        .filter_map(|(version, lts)| {
            let parts = numeric_selector(version).ok()?;
            if parts.len() != 3 || (selector == "lts/*" && !lts) {
                return None;
            }
            if numeric
                .as_ref()
                .is_some_and(|wanted| !parts.starts_with(wanted))
            {
                return None;
            }
            Some((
                parts,
                version.strip_prefix('v').unwrap_or(version).to_owned(),
            ))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, version)| version)
        .ok_or_else(|| {
            format!("Node {selector} is not installed — run `yerd install tool node {selector}`")
        })
}

fn exact_parts(version: &str) -> Option<Vec<u32>> {
    numeric_selector(version).ok().filter(|p| p.len() == 3)
}

fn strip_v(version: &str) -> &str {
    version.strip_prefix('v').unwrap_or(version)
}

/// A per-project Node version file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectFileKind {
    /// `.nvmrc` (nvm).
    Nvmrc,
    /// `.node-version` (nodenv, fnm, and others).
    NodeVersion,
}

impl ProjectFileKind {
    /// The file name looked up in each directory.
    #[must_use]
    pub fn file_name(self) -> &'static str {
        match self {
            Self::Nvmrc => ".nvmrc",
            Self::NodeVersion => ".node-version",
        }
    }
}

/// Lookup order within one directory: `.nvmrc` wins over `.node-version`.
pub const SEARCH_ORDER: [ProjectFileKind; 2] =
    [ProjectFileKind::Nvmrc, ProjectFileKind::NodeVersion];

/// Parse a project file's contents into a selector.
///
/// # Errors
/// Rejects empty contents and anything other than `node`, `lts/*`, or a
/// numeric selector.
pub fn parse_project_selector(contents: &str) -> Result<String, String> {
    let value = contents.trim();
    if !matches!(value, "node" | "lts/*") {
        numeric_selector(value)?;
    }
    Ok(value.to_owned())
}

/// Where an effective Node selection came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NodeSource {
    /// A `.nvmrc` file in the project.
    Nvmrc,
    /// A `.node-version` file in the project.
    NodeVersion,
    /// The site's saved preference.
    Site,
    /// The global default.
    Default,
}

impl NodeSource {
    /// The wire and CLI spelling (`nvmrc`, `node-version`, `site`, `default`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Nvmrc => "nvmrc",
            Self::NodeVersion => "node-version",
            Self::Site => "site",
            Self::Default => "default",
        }
    }
}

impl From<ProjectFileKind> for NodeSource {
    fn from(kind: ProjectFileKind) -> Self {
        match kind {
            ProjectFileKind::Nvmrc => Self::Nvmrc,
            ProjectFileKind::NodeVersion => Self::NodeVersion,
        }
    }
}

/// Pick the selector that applies: a project file, then the site preference,
/// then the global default.
#[must_use]
pub fn choose<'a>(
    file: Option<(&'a str, ProjectFileKind)>,
    site: Option<&'a str>,
    default: Option<&'a str>,
) -> Option<(&'a str, NodeSource)> {
    if let Some((selector, kind)) = file {
        return Some((selector, NodeSource::from(kind)));
    }
    site.map(|s| (s, NodeSource::Site))
        .or_else(|| default.map(|d| (d, NodeSource::Default)))
}

/// Oldest major offered for install. npx first shipped with npm 5.2 in Node
/// 8.2, so part of the 8.x line lacks it; from 9 on every release has it.
pub const MIN_LISTED_MAJOR: u32 = 9;

/// Whether an index release has a build for this host and is new enough.
#[must_use]
pub fn installable(files: &[&str], host_key: &str, major: u32) -> bool {
    major >= MIN_LISTED_MAJOR && files.contains(&host_key)
}

/// Major number of a nodejs/Release `schedule.json` key such as `"v22"`.
/// Pre-1.0 keys like `"v0.12"` have no single major and return `None`.
#[must_use]
pub fn schedule_major(key: &str) -> Option<u32> {
    let digits = key.strip_prefix('v')?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok().filter(|m| *m > 0)
}

/// One entry of the nodejs.org release index, borrowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexRelease<'a> {
    /// Version, with or without a leading `v`.
    pub version: &'a str,
    /// LTS codename, when the release is an LTS release.
    pub lts: Option<&'a str>,
}

/// One Node major line in the browsable release list.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MajorLine {
    /// Major version number.
    pub major: u32,
    /// Newest published patch, without a leading `v`.
    pub latest: String,
    /// LTS codename of the newest release in this line that has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codename: Option<String>,
    /// Whether the line is past its end of life; `None` when unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eol: Option<bool>,
    /// Highest installed patch in this line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed_latest: Option<String>,
    /// Whether `installed_latest` is older than `latest`.
    pub update_available: bool,
}

/// Group index releases by major, newest major first.
///
/// `schedule` maps a major to its `YYYY-MM-DD` end-of-life date and `today`
/// uses the same format, so dates compare lexicographically.
#[must_use]
pub fn major_lines<'a>(
    releases: impl IntoIterator<Item = IndexRelease<'a>>,
    installed: &[&str],
    schedule: Option<&BTreeMap<u32, String>>,
    today: &str,
) -> Vec<MajorLine> {
    let mut groups: BTreeMap<u32, Line> = BTreeMap::new();
    for release in releases {
        let Some(parts) = exact_parts(release.version) else {
            continue;
        };
        let Some(&major) = parts.first() else {
            continue;
        };
        let version = strip_v(release.version);
        let codename = release.lts.map(str::trim).filter(|c| !c.is_empty());
        let line = groups.entry(major).or_insert_with(|| Line {
            latest: (parts.clone(), version.to_owned()),
            codename: None,
        });
        if let Some(name) = codename {
            if line
                .codename
                .as_ref()
                .map_or(true, |(best, _)| parts > *best)
            {
                line.codename = Some((parts.clone(), name.to_owned()));
            }
        }
        if parts > line.latest.0 {
            line.latest = (parts, version.to_owned());
        }
    }
    groups
        .into_iter()
        .rev()
        .map(|(major, line)| {
            let installed_latest = installed
                .iter()
                .filter_map(|v| exact_parts(v).map(|p| (p, strip_v(v))))
                .filter(|(p, _)| p.first() == Some(&major))
                .max_by(|a, b| a.0.cmp(&b.0));
            MajorLine {
                major,
                codename: line.codename.map(|(_, name)| name),
                eol: schedule
                    .and_then(|s| s.get(&major))
                    .map(|end| today > end.as_str()),
                update_available: installed_latest
                    .as_ref()
                    .is_some_and(|(p, _)| *p < line.latest.0),
                installed_latest: installed_latest.map(|(_, v)| v.to_owned()),
                latest: line.latest.1,
            }
        })
        .collect()
}

/// Installed releases with a newer published patch, keyed by installed version.
///
/// `latest` maps each major to its newest known patch (from the last index
/// fetch). Only the highest installed patch of a major is reported, so an older
/// patch kept beside a newer one never counts as a second update.
#[must_use]
pub fn release_updates(
    installed: &[&str],
    latest: &BTreeMap<u32, String>,
) -> BTreeMap<String, String> {
    major_lines(
        latest
            .values()
            .map(|version| IndexRelease { version, lts: None }),
        installed,
        None,
        "",
    )
    .into_iter()
    .filter(|line| line.update_available)
    .filter_map(|line| Some((line.installed_latest?, line.latest)))
    .collect()
}

struct Line {
    latest: (Vec<u32>, String),
    codename: Option<(Vec<u32>, String)>,
}

/// The new global default after an Update installed `new_exact`, if it moves.
///
/// It moves only when `default` resolved to an older patch of the same major
/// before the update and does not already resolve to `new_exact` now (a
/// selector like `"22"` follows the newest patch on its own).
#[must_use]
pub fn moved_default(
    default: Option<&str>,
    installed_after: &[(&str, bool)],
    new_exact: &str,
) -> Option<String> {
    let default = default?;
    let new_exact = strip_v(new_exact);
    let new_parts = exact_parts(new_exact)?;
    if select_release(default, installed_after.iter().copied()).ok()? == new_exact {
        return None;
    }
    let before = select_release(
        default,
        installed_after
            .iter()
            .copied()
            .filter(|(v, _)| strip_v(v) != new_exact),
    )
    .ok()?;
    let before_parts = exact_parts(&before)?;
    (before_parts.first() == new_parts.first() && before_parts < new_parts)
        .then(|| new_exact.to_owned())
}

/// Why a release cannot be uninstalled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum UninstallRefusal {
    /// The release is not installed.
    #[error("not installed")]
    NotInstalled,
    /// The global default resolves only to this release.
    #[error("it is the global default - set another version as default first")]
    IsDefault,
}

/// Config changes that accompany an uninstall.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UninstallPlan {
    /// The default selector to persist; `None` clears it.
    pub default: Option<String>,
    /// Project roots whose saved preference is cleared.
    pub cleared: Vec<String>,
}

/// Decide what uninstalling `target` does to the default and site preferences.
///
/// Every comparison resolves selectors against the installed set before and
/// after the removal. Removing the last release clears the default. Otherwise
/// the uninstall is refused when the default resolves to the target before and
/// to nothing after. A preference is cleared under the same condition.
///
/// # Errors
/// [`UninstallRefusal::NotInstalled`] when `target` is not in `installed`,
/// [`UninstallRefusal::IsDefault`] when the default would dangle.
pub fn plan_uninstall(
    target: &str,
    installed: &[(&str, bool)],
    default: Option<&str>,
    prefs: &BTreeMap<String, String>,
) -> Result<UninstallPlan, UninstallRefusal> {
    let target = strip_v(target);
    if !installed.iter().any(|(v, _)| strip_v(v) == target) {
        return Err(UninstallRefusal::NotInstalled);
    }
    let after: Vec<(&str, bool)> = installed
        .iter()
        .copied()
        .filter(|(v, _)| strip_v(v) != target)
        .collect();
    let dangles = |selector: &str| {
        select_release(selector, installed.iter().copied()).is_ok_and(|v| v == target)
            && select_release(selector, after.iter().copied()).is_err()
    };
    let default = if after.is_empty() {
        None
    } else if default.is_some_and(dangles) {
        return Err(UninstallRefusal::IsDefault);
    } else {
        default.map(str::to_owned)
    };
    let cleared = prefs
        .iter()
        .filter(|(_, selector)| dangles(selector))
        .map(|(root, _)| root.clone())
        .collect();
    Ok(UninstallPlan { default, cleared })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    #[test]
    fn numeric_selection_is_order_independent() {
        let versions = [
            ("24.2.0", true),
            ("24.11.0", true),
            ("22.9.0", true),
            ("25.1.0", false),
        ];
        for (selector, expected) in [
            ("v24", "24.11.0"),
            ("24.2", "24.2.0"),
            ("v22.9.0", "22.9.0"),
            ("node", "25.1.0"),
            ("lts/*", "24.11.0"),
        ] {
            assert_eq!(select_release(selector, versions).unwrap(), expected);
        }
        for invalid in [
            "",
            "lts/jod",
            "24.x",
            "../24",
            "24.1.0.1",
            "24\n22",
            "24.999999999999",
        ] {
            assert!(select_release(invalid, versions).is_err());
        }
        assert!(select_release("23", versions)
            .unwrap_err()
            .contains("yerd install tool node 23"));
    }

    #[test]
    fn project_files_are_searched_nvmrc_first() {
        assert_eq!(
            SEARCH_ORDER.map(ProjectFileKind::file_name),
            [".nvmrc", ".node-version"]
        );
    }

    #[test]
    fn project_selectors_parse_or_reject() {
        for (input, expected) in [
            ("22\n", Some("22")),
            ("  v22.9.0 ", Some("v22.9.0")),
            ("node", Some("node")),
            ("lts/*\n", Some("lts/*")),
            ("", None),
            ("   \n", None),
            ("lts/jod", None),
            ("22.x", None),
        ] {
            assert_eq!(
                parse_project_selector(input).ok().as_deref(),
                expected,
                "{input:?}"
            );
        }
    }

    #[test]
    fn node_source_strings_match_serde() {
        for (source, text) in [
            (NodeSource::Nvmrc, "nvmrc"),
            (NodeSource::NodeVersion, "node-version"),
            (NodeSource::Site, "site"),
            (NodeSource::Default, "default"),
        ] {
            assert_eq!(source.as_str(), text);
        }
        assert_eq!(NodeSource::from(ProjectFileKind::Nvmrc), NodeSource::Nvmrc);
        assert_eq!(
            NodeSource::from(ProjectFileKind::NodeVersion),
            NodeSource::NodeVersion
        );
    }

    #[test]
    fn choose_prefers_file_then_site_then_default() {
        let file = Some(("18", ProjectFileKind::NodeVersion));
        for (file, site, default, expected) in [
            (
                file,
                Some("22"),
                Some("24"),
                Some(("18", NodeSource::NodeVersion)),
            ),
            (None, Some("22"), Some("24"), Some(("22", NodeSource::Site))),
            (None, None, Some("24"), Some(("24", NodeSource::Default))),
            (None, None, None, None),
        ] {
            assert_eq!(choose(file, site, default), expected);
        }
    }

    #[test]
    fn installable_needs_host_build_and_major_floor() {
        let files = ["linux-x64", "osx-arm64-tar"];
        for (key, major, expected) in [
            ("linux-x64", 22, true),
            ("osx-arm64-tar", 9, true),
            ("linux-x64", 8, false),
            ("linux-arm64", 22, false),
        ] {
            assert_eq!(installable(&files, key, major), expected, "{key} {major}");
        }
    }

    #[test]
    fn schedule_keys_map_to_majors() {
        for (key, expected) in [
            ("v22", Some(22)),
            ("v4", Some(4)),
            ("v0.12", None),
            ("v0.10", None),
            ("22", None),
            ("v", None),
            ("vx", None),
        ] {
            assert_eq!(schedule_major(key), expected, "{key}");
        }
    }

    fn release(version: &'static str, lts: Option<&'static str>) -> IndexRelease<'static> {
        IndexRelease { version, lts }
    }

    #[test]
    fn major_lines_group_newest_first() {
        let releases = [
            release("v22.9.0", Some("Jod")),
            release("v24.1.0", None),
            release("v22.12.0", Some("Jod")),
            release("v20.1.0", None),
            release("v20.19.0", Some("Iron")),
            release("v20.18.0", Some("Iron")),
            release("bogus", None),
        ];
        let schedule =
            BTreeMap::from([(20, "2026-04-30".to_owned()), (22, "2027-04-30".to_owned())]);
        let lines = major_lines(
            releases,
            &["22.9.0", "20.19.0", "18.1.0"],
            Some(&schedule),
            "2026-11-17",
        );
        assert_eq!(
            lines,
            vec![
                MajorLine {
                    major: 24,
                    latest: "24.1.0".into(),
                    codename: None,
                    eol: None,
                    installed_latest: None,
                    update_available: false,
                },
                MajorLine {
                    major: 22,
                    latest: "22.12.0".into(),
                    codename: Some("Jod".into()),
                    eol: Some(false),
                    installed_latest: Some("22.9.0".into()),
                    update_available: true,
                },
                MajorLine {
                    major: 20,
                    latest: "20.19.0".into(),
                    codename: Some("Iron".into()),
                    eol: Some(true),
                    installed_latest: Some("20.19.0".into()),
                    update_available: false,
                },
            ]
        );
    }

    #[test]
    fn major_lines_without_schedule_have_unknown_eol() {
        let lines = major_lines([release("v22.1.0", None)], &[], None, "2026-11-17");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].eol, None);
    }

    #[test]
    fn release_updates_report_only_the_newest_installed_patch_per_major() {
        let latest: BTreeMap<u32, String> = [
            (22, "22.12.0".to_owned()),
            (20, "20.18.0".to_owned()),
            (24, "v24.3.0".to_owned()),
        ]
        .into_iter()
        .collect();
        for (installed, expected) in [
            (vec!["22.9.0"], vec![("22.9.0", "22.12.0")]),
            (vec!["22.9.0", "22.11.0"], vec![("22.11.0", "22.12.0")]),
            (vec!["22.12.0", "22.9.0"], vec![]),
            (vec!["20.18.0"], vec![]),
            (vec!["v24.1.0", "18.0.0"], vec![("24.1.0", "24.3.0")]),
            (vec!["not-a-version"], vec![]),
            (vec![], vec![]),
        ] {
            let expected: BTreeMap<String, String> = expected
                .into_iter()
                .map(|(a, b)| (a.to_owned(), b.to_owned()))
                .collect();
            assert_eq!(
                release_updates(&installed, &latest),
                expected,
                "{installed:?}"
            );
        }
    }

    #[test]
    fn moved_default_only_moves_older_patches_of_the_major() {
        let installed = [("22.9.0", true), ("22.12.0", true), ("24.1.0", false)];
        for (default, expected) in [
            (Some("22"), None),
            (Some("22.9.0"), Some("22.12.0")),
            (Some("22.9"), Some("22.12.0")),
            (Some("24.1.0"), None),
            (Some("node"), None),
            (Some("99"), None),
            (None, None),
        ] {
            assert_eq!(
                moved_default(default, &installed, "22.12.0").as_deref(),
                expected,
                "{default:?}"
            );
        }
    }

    #[test]
    #[allow(clippy::type_complexity)]
    fn plan_uninstall_resolves_selectors_before_and_after() {
        let two_22 = [("22.11.0", true), ("22.12.0", true)];
        let partial = [("22.11.0", true), ("24.1.0", false)];
        let only = [("22.11.0", true)];
        let prefs = BTreeMap::from([
            ("/srv/a".to_owned(), "22".to_owned()),
            ("/srv/b".to_owned(), "22.11.0".to_owned()),
            ("/srv/c".to_owned(), "24".to_owned()),
        ]);
        let cases: [(
            &str,
            &[(&str, bool)],
            Option<&str>,
            Result<UninstallPlan, UninstallRefusal>,
        ); 7] = [
            (
                "22.11.0",
                &two_22,
                Some("22"),
                Ok(UninstallPlan {
                    default: Some("22".into()),
                    cleared: vec!["/srv/b".into()],
                }),
            ),
            (
                "22.12.0",
                &two_22,
                Some("22"),
                Ok(UninstallPlan {
                    default: Some("22".into()),
                    cleared: vec![],
                }),
            ),
            (
                "22.11.0",
                &partial,
                Some("22.11"),
                Err(UninstallRefusal::IsDefault),
            ),
            (
                "v22.11.0",
                &partial,
                Some("24"),
                Ok(UninstallPlan {
                    default: Some("24".into()),
                    cleared: vec!["/srv/a".into(), "/srv/b".into()],
                }),
            ),
            (
                "22.11.0",
                &only,
                Some("22.11.0"),
                Ok(UninstallPlan {
                    default: None,
                    cleared: vec!["/srv/a".into(), "/srv/b".into()],
                }),
            ),
            (
                "20.1.0",
                &partial,
                Some("24"),
                Err(UninstallRefusal::NotInstalled),
            ),
            (
                "24.1.0",
                &partial,
                None,
                Ok(UninstallPlan {
                    default: None,
                    cleared: vec!["/srv/c".into()],
                }),
            ),
        ];
        for (target, installed, default, expected) in cases {
            assert_eq!(
                plan_uninstall(target, installed, default, &prefs),
                expected,
                "{target} {default:?}"
            );
        }
    }
}
