//! Pure metadata for supported IDE launchers.

use std::path::{Path, PathBuf};

/// Executable and application names used to detect one IDE on each OS.
pub struct IdeSpec {
    /// Stable IDE identifier, used as the persisted and wire form.
    pub id: &'static str,
    /// User-facing name.
    pub display_name: &'static str,
    /// Auto-detect preference; the lowest rank among detected IDEs wins.
    pub rank: u8,
    /// CLI executable names, checked in `PATH`.
    pub cli_names: &'static [&'static str],
    /// macOS application names, checked in the standard application roots.
    pub mac_app_names: &'static [&'static str],
    /// Linux desktop-entry application names, matched case-insensitively.
    pub linux_desktop_names: &'static [&'static str],
    /// Linux desktop-entry IDs, without the `.desktop` suffix.
    pub linux_desktop_ids: &'static [&'static str],
}

/// Supported IDE launch metadata in `rank` order.
///
/// VS Code deliberately has no bare `"Code"` desktop name: distro packages that
/// call themselves `Name=Code` are still matched through `linux_desktop_ids` and
/// the `Exec` command, while unrelated applications whose name merely starts
/// with "Code" no longer match.
pub const IDE_SPECS: &[IdeSpec] = &[
    IdeSpec {
        id: "phpstorm",
        display_name: "PhpStorm",
        rank: 0,
        cli_names: &["phpstorm"],
        mac_app_names: &["PhpStorm"],
        linux_desktop_names: &["PhpStorm"],
        linux_desktop_ids: &["phpstorm", "jetbrains-phpstorm"],
    },
    IdeSpec {
        id: "cursor",
        display_name: "Cursor",
        rank: 1,
        cli_names: &["cursor"],
        mac_app_names: &["Cursor"],
        linux_desktop_names: &["Cursor"],
        linux_desktop_ids: &["cursor", "com.todesktop.230313mzl4w4u92"],
    },
    IdeSpec {
        id: "windsurf",
        display_name: "Windsurf",
        rank: 2,
        cli_names: &["windsurf"],
        mac_app_names: &["Windsurf"],
        linux_desktop_names: &["Windsurf"],
        linux_desktop_ids: &["windsurf"],
    },
    IdeSpec {
        id: "zed",
        display_name: "Zed",
        rank: 3,
        cli_names: &["zed", "zeditor"],
        mac_app_names: &["Zed"],
        linux_desktop_names: &["Zed"],
        linux_desktop_ids: &["zed", "dev.zed.Zed"],
    },
    IdeSpec {
        id: "vscode",
        display_name: "VS Code",
        rank: 4,
        cli_names: &["code", "code-insiders"],
        mac_app_names: &["Visual Studio Code"],
        linux_desktop_names: &["Visual Studio Code", "VS Code"],
        linux_desktop_ids: &["code", "com.visualstudio.code", "visual-studio-code"],
    },
    IdeSpec {
        id: "vscodium",
        display_name: "VSCodium",
        rank: 5,
        cli_names: &["codium", "vscodium"],
        mac_app_names: &["VSCodium"],
        linux_desktop_names: &["VSCodium"],
        linux_desktop_ids: &["codium", "com.vscodium.codium"],
    },
    IdeSpec {
        id: "sublime",
        display_name: "Sublime Text",
        rank: 6,
        cli_names: &["subl", "sublime_text"],
        mac_app_names: &["Sublime Text", "Sublime Text 4"],
        linux_desktop_names: &["Sublime Text"],
        linux_desktop_ids: &["sublime_text", "sublime-text"],
    },
];

/// Find metadata for one supported IDE id.
#[must_use]
pub fn spec_for(id: &str) -> Option<&'static IdeSpec> {
    IDE_SPECS.iter().find(|spec| spec.id == id)
}

/// Return extra macOS CLI directories used when the GUI has no shell `PATH`.
#[must_use]
pub fn ide_cli_candidates_macos(home: Option<&Path>) -> Vec<PathBuf> {
    let mut candidates = vec![
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/opt/homebrew/bin"),
    ];
    if let Some(home) = home {
        candidates.push(home.join("Library/Application Support/JetBrains/Toolbox/scripts"));
    }
    candidates
}

/// Return extra Windows CLI directories used when an editor's launcher is not
/// on `PATH`.
///
/// Only the `JetBrains` Toolbox scripts directory, the direct analogue of the
/// entry both sibling functions already carry. Empty when `%LOCALAPPDATA%` is
/// unset. Windows installers otherwise put their launcher on `PATH`, and a
/// Windows GUI process inherits the full user and machine `PATH`, so the
/// stripped-`PATH` problem that motivates the longer Unix lists barely applies.
#[must_use]
pub fn ide_cli_candidates_windows(local_app_data: Option<&Path>) -> Vec<PathBuf> {
    local_app_data
        .map(|d| vec![d.join("JetBrains").join("Toolbox").join("scripts")])
        .unwrap_or_default()
}

/// Filenames to probe for an editor CLI called `cli_name` on Windows.
///
/// `.exe` first so a native executable wins over a shim that has to route
/// through `cmd.exe`. [`IdeSpec::cli_names`] needs no Windows column because
/// every supported editor's Windows launcher is one of these three extensions
/// over the same stem.
#[must_use]
pub fn windows_executable_names(cli_name: &str) -> Vec<String> {
    ["exe", "cmd", "bat"]
        .iter()
        .map(|ext| format!("{cli_name}.{ext}"))
        .collect()
}

/// Return extra Linux CLI directories used when the GUI has no shell `PATH`.
/// Covers the Flatpak, Snap, Nix, and `JetBrains` Toolbox export directories a
/// desktop-launched process does not inherit.
#[must_use]
pub fn ide_cli_candidates_linux(home: Option<&Path>) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(home) = home {
        candidates.push(home.join(".local/bin"));
    }
    candidates.push(PathBuf::from("/var/lib/flatpak/exports/bin"));
    if let Some(home) = home {
        candidates.push(home.join(".local/share/flatpak/exports/bin"));
    }
    candidates.push(PathBuf::from("/snap/bin"));
    if let Some(home) = home {
        candidates.push(home.join(".nix-profile/bin"));
    }
    candidates.push(PathBuf::from("/run/current-system/sw/bin"));
    if let Some(home) = home {
        candidates.push(home.join(".local/share/JetBrains/Toolbox/scripts"));
    }
    candidates
}

/// Return the macOS application roots scanned for IDE bundles.
#[must_use]
pub fn mac_application_locations(home: Option<&Path>) -> Vec<PathBuf> {
    let mut locations = vec![
        PathBuf::from("/Applications"),
        PathBuf::from("/System/Applications"),
        PathBuf::from("/Network/Applications"),
    ];
    if let Some(home) = home {
        locations.push(home.join("Applications"));
        locations.push(home.join("Library/Application Support/JetBrains/Toolbox/apps"));
    }
    locations
}

/// Return whether a desktop-entry `Name` identifies the selected IDE.
#[must_use]
pub fn desktop_name_matches(id: &str, name: &str) -> bool {
    spec_for(id).is_some_and(|spec| {
        spec.linux_desktop_names
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(name.trim()))
    })
}

fn mac_preview_label_matches(value: &str) -> bool {
    ["Beta", "Canary", "EAP", "Insiders", "Nightly", "Preview"]
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(value))
}

fn mac_version_matches(value: &str) -> bool {
    let mut components = value.split('.');
    components.next().is_some_and(|component| {
        !component.is_empty()
            && component
                .chars()
                .all(|character| character.is_ascii_digit())
    }) && components.all(|component| {
        !component.is_empty()
            && component
                .chars()
                .all(|character| character.is_ascii_digit())
    })
}

fn mac_app_suffix_matches(suffix: &str) -> bool {
    let Some(suffix) = suffix.strip_prefix(' ') else {
        return false;
    };
    let mut words = suffix.split_whitespace();
    let Some(first) = words.next() else {
        return false;
    };
    if mac_version_matches(first) {
        return match words.next() {
            None => true,
            Some(label) if mac_preview_label_matches(label) => words.next().is_none(),
            Some(_) => false,
        };
    }
    if words.next().is_none() && mac_preview_label_matches(first) {
        return true;
    }
    let Some(label) = suffix.strip_prefix("- ") else {
        return false;
    };
    mac_preview_label_matches(label)
}

/// Return whether a macOS application bundle name identifies the selected IDE.
/// Versioned and preview bundle names may add a suffix after the known name:
/// `PhpStorm 2025.1`, `PhpStorm 2025.1 EAP`, `Visual Studio Code - Insiders`,
/// and the bare-label form Zed ships its preview channel under, `Zed Preview`.
#[must_use]
pub fn mac_app_name_matches(id: &str, name: &str) -> bool {
    let name = name.trim();
    spec_for(id).is_some_and(|spec| {
        spec.mac_app_names.iter().any(|candidate| {
            if name.eq_ignore_ascii_case(candidate) {
                return true;
            }
            let Some(prefix) = name.get(..candidate.len()) else {
                return false;
            };
            let Some(suffix) = name.get(candidate.len()..) else {
                return false;
            };
            prefix.eq_ignore_ascii_case(candidate) && mac_app_suffix_matches(suffix)
        })
    })
}

fn desktop_id_matches(id: &str, file_name: &str) -> bool {
    let entry_id = file_name
        .trim()
        .strip_suffix(".desktop")
        .unwrap_or(file_name)
        .rsplit('/')
        .next()
        .unwrap_or_default();
    spec_for(id).is_some_and(|spec| {
        spec.linux_desktop_ids
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(entry_id))
    })
}

fn executable_name(value: &str) -> Option<&str> {
    let value = value.trim();
    let executable = if let Some(quoted) = value.strip_prefix('"') {
        quoted.split_once('"').map_or(quoted, |(value, _)| value)
    } else if let Some(quoted) = value.strip_prefix('\'') {
        quoted.split_once('\'').map_or(quoted, |(value, _)| value)
    } else {
        value.split_whitespace().next().unwrap_or_default()
    };
    executable
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
}

fn desktop_exec_matches(id: &str, exec: &str) -> bool {
    let Some(executable) = executable_name(exec) else {
        return false;
    };
    spec_for(id).is_some_and(|spec| {
        spec.cli_names
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(executable))
    })
}

/// Return whether desktop-entry text describes the selected IDE.
#[must_use]
pub fn desktop_entry_matches(id: &str, file_name: &str, text: &str) -> bool {
    let mut in_desktop_entry = false;
    let mut is_application = false;
    let mut name = None;
    let mut exec = None;

    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            in_desktop_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_desktop_entry {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "Type" => is_application = value.trim() == "Application",
            "Name" => name = Some(value.trim()),
            "Exec" => exec = Some(value.trim()),
            _ => {}
        }
    }

    is_application
        && (name.is_some_and(|value| desktop_name_matches(id, value))
            || desktop_id_matches(id, file_name)
            || exec.is_some_and(|value| desktop_exec_matches(id, value)))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn rank_order_is_the_documented_auto_detect_preference() {
        let mut ordered: Vec<&IdeSpec> = IDE_SPECS.iter().collect();
        ordered.sort_by_key(|spec| spec.rank);
        let ids: Vec<&str> = ordered.iter().map(|spec| spec.id).collect();
        assert_eq!(
            ids,
            vec!["phpstorm", "cursor", "windsurf", "zed", "vscode", "vscodium", "sublime"]
        );
    }

    #[test]
    fn ranks_and_ids_are_unique() {
        let mut ranks: Vec<u8> = IDE_SPECS.iter().map(|spec| spec.rank).collect();
        ranks.sort_unstable();
        let count = ranks.len();
        ranks.dedup();
        assert_eq!(ranks.len(), count);

        let mut ids: Vec<&str> = IDE_SPECS.iter().map(|spec| spec.id).collect();
        ids.sort_unstable();
        let count = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), count);
    }

    #[test]
    fn every_spec_is_reachable_by_id() {
        for spec in IDE_SPECS {
            assert!(spec_for(spec.id).is_some());
        }
        assert!(spec_for("system").is_none());
    }

    #[test]
    fn desktop_entries_match_only_the_application_group() {
        let zed = "# comment\n[Desktop Entry]\nType=Application\nName=Zed\nExec=zeditor %U\n";
        assert!(desktop_entry_matches("zed", "dev.zed.Zed.desktop", zed));
        assert!(!desktop_entry_matches("vscode", "dev.zed.Zed.desktop", zed));

        let wrong_type = "[Desktop Entry]\nType=Link\nName=Zed\n";
        assert!(!desktop_entry_matches(
            "zed",
            "dev.zed.Zed.desktop",
            wrong_type
        ));
    }

    #[test]
    fn desktop_entries_match_known_ids_and_exec_commands() {
        let vscode = "[Desktop Entry]\nType=Application\nName=Code Editor\nExec=/usr/bin/code %F\n";
        assert!(desktop_entry_matches("vscode", "code.desktop", vscode));

        let phpstorm = "[Desktop Entry]\nType=Application\nName=JetBrains IDE\nExec=\"/opt/PhpStorm/bin/phpstorm\" %f\n";
        assert!(desktop_entry_matches(
            "phpstorm",
            "jetbrains-phpstorm.desktop",
            phpstorm
        ));

        let unrelated = "[Desktop Entry]\nType=Application\nName=Codecs\nExec=codecs\n";
        assert!(!desktop_entry_matches(
            "vscode",
            "codecs.desktop",
            unrelated
        ));
    }

    #[test]
    fn bare_code_name_matches_only_with_a_vs_code_exec_or_id() {
        let packaged = "[Desktop Entry]\nType=Application\nName=Code\nExec=code %F\n";
        assert!(desktop_entry_matches(
            "vscode",
            "unrelated.desktop",
            packaged
        ));

        let impostor = "[Desktop Entry]\nType=Application\nName=Code\nExec=some-other-editor %F\n";
        assert!(!desktop_entry_matches(
            "vscode",
            "unrelated.desktop",
            impostor
        ));
    }

    #[test]
    fn vscodium_matches_its_names_ids_and_exec_commands() {
        let by_name = "[Desktop Entry]\nType=Application\nName=VSCodium\nExec=/usr/bin/codium %F\n";
        assert!(desktop_entry_matches(
            "vscodium",
            "unrelated.desktop",
            by_name
        ));

        let by_id = "[Desktop Entry]\nType=Application\nName=Editor\nExec=editor\n";
        assert!(desktop_entry_matches(
            "vscodium",
            "com.vscodium.codium.desktop",
            by_id
        ));

        assert!(mac_app_name_matches("vscodium", "VSCodium"));
        assert!(!mac_app_name_matches("vscodium", "VSCodium Backup"));
    }

    #[test]
    fn mac_app_names_match_versioned_and_preview_bundles() {
        let cases = [
            ("phpstorm", "PhpStorm 2025.1", true),
            ("phpstorm", "PhpStorm 2025.1 EAP", true),
            ("vscode", "Visual Studio Code - Insiders", true),
            ("zed", "Zed Preview", true),
            ("zed", "Zed Nightly", true),
            ("cursor", "Cursor Nightly", true),
            ("vscode", "Codecs", false),
            ("vscode", "Visual Studio Code - Backup", false),
            ("zed", "Zed Backup", false),
            ("zed", "Zed Preview Copy", false),
            ("zed", "Zed Old Preview", false),
            ("cursor", "Cursor 2 Backup", false),
        ];

        for (id, name, expected) in cases {
            assert_eq!(
                mac_app_name_matches(id, name),
                expected,
                "unexpected match result for {name}"
            );
        }
    }

    #[test]
    fn macos_cli_candidates_include_common_gui_launch_paths() {
        let home = PathBuf::from("/Users/test");
        assert_eq!(
            ide_cli_candidates_macos(Some(&home)),
            vec![
                PathBuf::from("/usr/local/bin"),
                PathBuf::from("/opt/homebrew/bin"),
                PathBuf::from("/Users/test/Library/Application Support/JetBrains/Toolbox/scripts"),
            ]
        );
        assert_eq!(
            ide_cli_candidates_macos(None),
            vec![
                PathBuf::from("/usr/local/bin"),
                PathBuf::from("/opt/homebrew/bin")
            ]
        );
    }

    #[test]
    fn linux_cli_candidates_include_common_gui_launch_paths() {
        let home = PathBuf::from("/home/test");
        assert_eq!(
            ide_cli_candidates_linux(Some(&home)),
            vec![
                PathBuf::from("/home/test/.local/bin"),
                PathBuf::from("/var/lib/flatpak/exports/bin"),
                PathBuf::from("/home/test/.local/share/flatpak/exports/bin"),
                PathBuf::from("/snap/bin"),
                PathBuf::from("/home/test/.nix-profile/bin"),
                PathBuf::from("/run/current-system/sw/bin"),
                PathBuf::from("/home/test/.local/share/JetBrains/Toolbox/scripts"),
            ]
        );
        assert_eq!(
            ide_cli_candidates_linux(None),
            vec![
                PathBuf::from("/var/lib/flatpak/exports/bin"),
                PathBuf::from("/snap/bin"),
                PathBuf::from("/run/current-system/sw/bin"),
            ]
        );
    }

    #[test]
    fn macos_application_locations_include_supported_roots() {
        let home = PathBuf::from("/Users/test");
        assert_eq!(
            mac_application_locations(Some(&home)),
            vec![
                PathBuf::from("/Applications"),
                PathBuf::from("/System/Applications"),
                PathBuf::from("/Network/Applications"),
                PathBuf::from("/Users/test/Applications"),
                PathBuf::from("/Users/test/Library/Application Support/JetBrains/Toolbox/apps"),
            ]
        );
        assert_eq!(
            mac_application_locations(None),
            vec![
                PathBuf::from("/Applications"),
                PathBuf::from("/System/Applications"),
                PathBuf::from("/Network/Applications")
            ]
        );
    }

    /// Built with `join` on both sides so the assertion holds on every host:
    /// the separator this un-gated test sees is the *host's*, not Windows'.
    #[test]
    fn windows_candidates_are_the_toolbox_scripts_dir() {
        let local = Path::new("local-app-data");
        let dirs = ide_cli_candidates_windows(Some(local));
        assert_eq!(
            dirs,
            vec![local.join("JetBrains").join("Toolbox").join("scripts")]
        );
    }

    #[test]
    fn windows_candidates_are_empty_without_local_app_data() {
        assert!(ide_cli_candidates_windows(None).is_empty());
    }

    /// `.exe` must come first: a native launcher beats a `.cmd` shim that has to
    /// route through `cmd.exe`.
    #[test]
    fn windows_executable_names_prefer_exe() {
        assert_eq!(
            windows_executable_names("code"),
            vec!["code.exe", "code.cmd", "code.bat"]
        );
        assert_eq!(
            windows_executable_names("phpstorm"),
            vec!["phpstorm.exe", "phpstorm.cmd", "phpstorm.bat"]
        );
        assert_eq!(
            windows_executable_names("subl"),
            vec!["subl.exe", "subl.cmd", "subl.bat"]
        );
    }
}
