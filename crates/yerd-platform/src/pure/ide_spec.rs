//! Pure metadata for supported IDE launchers.

use crate::ide::Ide;

/// Executable and application names used to detect one IDE on each OS.
pub struct IdeSpec {
    /// Stable IDE identifier.
    pub ide: Ide,
    /// CLI executable names, checked in `PATH`.
    pub cli_names: &'static [&'static str],
    /// macOS application names, checked in standard folders and Spotlight.
    pub mac_app_names: &'static [&'static str],
    /// Linux desktop-entry application names, matched case-insensitively.
    pub linux_desktop_names: &'static [&'static str],
    /// Linux desktop-entry IDs, without the `.desktop` suffix.
    pub linux_desktop_ids: &'static [&'static str],
}

/// Supported IDE launch metadata in the site sidebar display order.
pub const IDE_SPECS: &[IdeSpec] = &[
    IdeSpec {
        ide: Ide::VsCode,
        cli_names: &["code", "code-insiders"],
        mac_app_names: &["Visual Studio Code"],
        linux_desktop_names: &["Visual Studio Code", "VS Code", "Code"],
        linux_desktop_ids: &["code", "com.visualstudio.code", "visual-studio-code"],
    },
    IdeSpec {
        ide: Ide::Cursor,
        cli_names: &["cursor"],
        mac_app_names: &["Cursor"],
        linux_desktop_names: &["Cursor"],
        linux_desktop_ids: &["cursor", "com.todesktop.230313mzl4w4u92"],
    },
    IdeSpec {
        ide: Ide::Zed,
        cli_names: &["zed", "zeditor"],
        mac_app_names: &["Zed"],
        linux_desktop_names: &["Zed"],
        linux_desktop_ids: &["zed", "dev.zed.Zed"],
    },
    IdeSpec {
        ide: Ide::Sublime,
        cli_names: &["subl", "sublime_text"],
        mac_app_names: &["Sublime Text", "Sublime Text 4"],
        linux_desktop_names: &["Sublime Text"],
        linux_desktop_ids: &["sublime_text", "sublime-text"],
    },
    IdeSpec {
        ide: Ide::PhpStorm,
        cli_names: &["phpstorm"],
        mac_app_names: &["PhpStorm"],
        linux_desktop_names: &["PhpStorm"],
        linux_desktop_ids: &["phpstorm", "jetbrains-phpstorm"],
    },
    IdeSpec {
        ide: Ide::Windsurf,
        cli_names: &["windsurf"],
        mac_app_names: &["Windsurf"],
        linux_desktop_names: &["Windsurf"],
        linux_desktop_ids: &["windsurf"],
    },
];

/// Find metadata for one supported IDE.
#[must_use]
pub fn spec_for(ide: Ide) -> Option<&'static IdeSpec> {
    IDE_SPECS.iter().find(|spec| spec.ide == ide)
}

/// Return whether a desktop-entry `Name` identifies the selected IDE.
#[must_use]
pub fn desktop_name_matches(ide: Ide, name: &str) -> bool {
    spec_for(ide).is_some_and(|spec| {
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
    let Some(label) = suffix.strip_prefix("- ") else {
        return false;
    };
    mac_preview_label_matches(label)
}

/// Return whether a macOS application bundle name identifies the selected IDE.
/// Versioned and preview bundle names may add a suffix after the known name.
#[must_use]
pub fn mac_app_name_matches(ide: Ide, name: &str) -> bool {
    let name = name.trim();
    spec_for(ide).is_some_and(|spec| {
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

fn desktop_id_matches(ide: Ide, file_name: &str) -> bool {
    let id = file_name
        .trim()
        .strip_suffix(".desktop")
        .unwrap_or(file_name)
        .rsplit('/')
        .next()
        .unwrap_or_default();
    spec_for(ide).is_some_and(|spec| {
        spec.linux_desktop_ids
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(id))
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

fn desktop_exec_matches(ide: Ide, exec: &str) -> bool {
    let Some(executable) = executable_name(exec) else {
        return false;
    };
    spec_for(ide).is_some_and(|spec| {
        spec.cli_names
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(executable))
    })
}

/// Return whether desktop-entry text describes the selected IDE.
#[must_use]
pub fn desktop_entry_matches(ide: Ide, file_name: &str, text: &str) -> bool {
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
        && (name.is_some_and(|value| desktop_name_matches(ide, value))
            || desktop_id_matches(ide, file_name)
            || exec.is_some_and(|value| desktop_exec_matches(ide, value)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_ide_has_metadata() {
        for ide in Ide::all() {
            assert!(spec_for(*ide).is_some());
        }
    }

    #[test]
    fn wire_names_round_trip() {
        for ide in Ide::all() {
            assert_eq!(Ide::from_wire(ide.wire_name()), Some(*ide));
        }
        assert_eq!(Ide::from_wire("system"), None);
    }

    #[test]
    fn desktop_entries_match_only_the_application_group() {
        let zed = "# comment\n[Desktop Entry]\nType=Application\nName=Zed\nExec=zeditor %U\n";
        assert!(desktop_entry_matches(Ide::Zed, "dev.zed.Zed.desktop", zed));
        assert!(!desktop_entry_matches(
            Ide::VsCode,
            "dev.zed.Zed.desktop",
            zed
        ));

        let wrong_type = "[Desktop Entry]\nType=Link\nName=Zed\n";
        assert!(!desktop_entry_matches(
            Ide::Zed,
            "dev.zed.Zed.desktop",
            wrong_type
        ));
    }

    #[test]
    fn desktop_entries_match_known_ids_and_exec_commands() {
        let vscode = "[Desktop Entry]\nType=Application\nName=Code Editor\nExec=/usr/bin/code %F\n";
        assert!(desktop_entry_matches(Ide::VsCode, "code.desktop", vscode));

        let phpstorm = "[Desktop Entry]\nType=Application\nName=JetBrains IDE\nExec=\"/opt/PhpStorm/bin/phpstorm\" %f\n";
        assert!(desktop_entry_matches(
            Ide::PhpStorm,
            "jetbrains-phpstorm.desktop",
            phpstorm
        ));

        let unrelated = "[Desktop Entry]\nType=Application\nName=Codecs\nExec=codecs\n";
        assert!(!desktop_entry_matches(
            Ide::VsCode,
            "codecs.desktop",
            unrelated
        ));
    }

    #[test]
    fn mac_app_names_match_versioned_and_preview_bundles() {
        assert!(mac_app_name_matches(Ide::PhpStorm, "PhpStorm 2025.1"));
        assert!(mac_app_name_matches(
            Ide::VsCode,
            "Visual Studio Code - Insiders"
        ));
        assert!(mac_app_name_matches(Ide::PhpStorm, "PhpStorm 2025.1 EAP"));
        assert!(!mac_app_name_matches(Ide::VsCode, "Codecs"));
        assert!(!mac_app_name_matches(
            Ide::VsCode,
            "Visual Studio Code - Backup"
        ));
    }
}
