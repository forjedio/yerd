//! Keep the project version in sync across the three manifests that declare it:
//! the workspace `Cargo.toml` (`[workspace.package].version`), the GUI's
//! `tauri.conf.json`, and the GUI's `package.json`.
//!
//! Pure string transforms live here (unit-tested table-style); the I/O wrappers
//! (`bump` / `version-check`) live in `main.rs`. We edit only the single version
//! line in each file - never reformat the whole document.

use anyhow::{bail, Result};

/// Replace the content of the **last** double-quoted string on `line` with
/// `value`, preserving everything else (indent, key, trailing comma). The value
/// is the last quoted string in both `version = "X"` (TOML) and
/// `"version": "X",` (JSON), so one helper serves both.
fn replace_last_string(line: &str, value: &str) -> Option<String> {
    let close = line.rfind('"')?;
    let open = line[..close].rfind('"')?;
    Some(format!("{}{value}{}", &line[..=open], &line[close..]))
}

/// The content of the last double-quoted string on `line`.
fn last_string(line: &str) -> Option<&str> {
    let close = line.rfind('"')?;
    let open = line[..close].rfind('"')?;
    Some(&line[open + 1..close])
}

/// Index of the `version` line inside the `[workspace.package]` table.
fn cargo_version_idx(toml: &str) -> Option<usize> {
    let mut in_pkg = false;
    for (i, line) in toml.lines().enumerate() {
        let t = line.trim_start();
        if t.starts_with('[') {
            in_pkg = t.starts_with("[workspace.package]");
            continue;
        }
        if in_pkg {
            if let Some(rest) = t.strip_prefix("version") {
                if rest.trim_start().starts_with('=') {
                    return Some(i);
                }
            }
        }
    }
    None
}

/// Index of the top-level `"version"` key line in a JSON manifest.
fn json_version_idx(json: &str) -> Option<usize> {
    json.lines()
        .position(|l| l.trim_start().starts_with("\"version\""))
}

/// Faithfully rewrite line `idx` of `content` to `new_line` (preserves the
/// original trailing-newline behaviour for `\n`-terminated files).
fn rewrite_line(content: &str, idx: usize, new_line: &str) -> String {
    let ends_nl = content.ends_with('\n');
    let lines: Vec<&str> = content.lines().collect();
    let mut out = String::with_capacity(content.len() + new_line.len());
    for (i, l) in lines.iter().enumerate() {
        if i == idx {
            out.push_str(new_line);
        } else {
            out.push_str(l);
        }
        if i + 1 < lines.len() || ends_nl {
            out.push('\n');
        }
    }
    out
}

/// Set `[workspace.package].version` in a `Cargo.toml`.
pub fn set_cargo(content: &str, version: &str) -> Result<String> {
    let idx = cargo_version_idx(content)
        .ok_or_else(|| anyhow::anyhow!("no `version` under [workspace.package]"))?;
    let line = content.lines().nth(idx).unwrap_or_default();
    let new_line = replace_last_string(line, version)
        .ok_or_else(|| anyhow::anyhow!("malformed version line: {line:?}"))?;
    Ok(rewrite_line(content, idx, &new_line))
}

/// Set the top-level `"version"` in a JSON manifest (tauri.conf.json / package.json).
pub fn set_json(content: &str, version: &str) -> Result<String> {
    let idx = json_version_idx(content)
        .ok_or_else(|| anyhow::anyhow!("no top-level `\"version\"` key"))?;
    let line = content.lines().nth(idx).unwrap_or_default();
    let new_line = replace_last_string(line, version)
        .ok_or_else(|| anyhow::anyhow!("malformed version line: {line:?}"))?;
    Ok(rewrite_line(content, idx, &new_line))
}

/// Read `[workspace.package].version` from a `Cargo.toml`.
pub fn get_cargo(content: &str) -> Result<String> {
    let idx = cargo_version_idx(content)
        .ok_or_else(|| anyhow::anyhow!("no `version` under [workspace.package]"))?;
    let line = content.lines().nth(idx).unwrap_or_default();
    last_string(line)
        .map(ToOwned::to_owned)
        .ok_or_else(|| anyhow::anyhow!("malformed version line: {line:?}"))
}

/// Read the top-level `"version"` from a JSON manifest.
pub fn get_json(content: &str) -> Result<String> {
    let idx = json_version_idx(content)
        .ok_or_else(|| anyhow::anyhow!("no top-level `\"version\"` key"))?;
    let line = content.lines().nth(idx).unwrap_or_default();
    last_string(line)
        .map(ToOwned::to_owned)
        .ok_or_else(|| anyhow::anyhow!("malformed version line: {line:?}"))
}

/// Normalise a release tag/version: strip a single leading `v`.
pub fn normalise(tag: &str) -> &str {
    tag.strip_prefix('v').unwrap_or(tag)
}

/// One manifest's name + found version, for reporting.
pub struct Found {
    pub label: &'static str,
    pub version: String,
}

/// Assert all three found versions equal `expected`. Returns a human-readable
/// error listing every mismatch when they don't.
pub fn assert_all_match(expected: &str, found: &[Found]) -> Result<()> {
    let bad: Vec<String> = found
        .iter()
        .filter(|f| f.version != expected)
        .map(|f| format!("  {} = {} (expected {})", f.label, f.version, expected))
        .collect();
    if bad.is_empty() {
        return Ok(());
    }
    bail!(
        "version mismatch — these manifests don't match `{expected}`:\n{}\n\
         Run `cargo xtask bump {expected}`, commit, then re-tag.",
        bad.join("\n")
    );
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    const CARGO: &str = "[workspace]\n\
        members = []\n\n\
        [workspace.package]\n\
        version      = \"2.0.1\"\n\
        edition      = \"2021\"\n\n\
        [workspace.dependencies]\n\
        serde = { version = \"1\" }\n";

    const TAURI: &str = "{\n  \"productName\": \"Yerd\",\n  \"version\": \"2.0.1\",\n  \"identifier\": \"dev.yerd.gui\"\n}\n";
    const PKG: &str =
        "{\n  \"name\": \"yerd-gui\",\n  \"version\": \"2.0.1\",\n  \"type\": \"module\"\n}\n";

    #[test]
    fn cargo_targets_workspace_package_not_dependencies() {
        let out = set_cargo(CARGO, "2.1.0").unwrap();
        assert!(
            out.contains("version      = \"2.1.0\""),
            "alignment preserved"
        );
        assert!(out.contains("serde = { version = \"1\" }"));
        assert_eq!(get_cargo(&out).unwrap(), "2.1.0");
    }

    #[test]
    fn json_value_and_trailing_comma_preserved() {
        let out = set_json(TAURI, "2.1.0").unwrap();
        assert!(out.contains("  \"version\": \"2.1.0\","));
        assert!(out.contains("\"productName\": \"Yerd\""));
        assert_eq!(get_json(&out).unwrap(), "2.1.0");

        let out2 = set_json(PKG, "2.1.0").unwrap();
        assert!(out2.contains("  \"version\": \"2.1.0\","));
        assert_eq!(get_json(&out2).unwrap(), "2.1.0");
    }

    #[test]
    fn prerelease_versions_round_trip() {
        let out = set_cargo(CARGO, "2.0.2-rc.1").unwrap();
        assert_eq!(get_cargo(&out).unwrap(), "2.0.2-rc.1");
        let out = set_json(PKG, "2.0.2-rc.1").unwrap();
        assert_eq!(get_json(&out).unwrap(), "2.0.2-rc.1");
    }

    #[test]
    fn reads_current_versions() {
        assert_eq!(get_cargo(CARGO).unwrap(), "2.0.1");
        assert_eq!(get_json(TAURI).unwrap(), "2.0.1");
        assert_eq!(get_json(PKG).unwrap(), "2.0.1");
    }

    #[test]
    fn trailing_newline_preserved() {
        assert!(set_cargo(CARGO, "9.9.9").unwrap().ends_with('\n'));
        let no_nl = "[workspace.package]\nversion = \"2.0.1\"";
        assert!(!set_cargo(no_nl, "9.9.9").unwrap().ends_with('\n'));
    }

    #[test]
    fn normalise_strips_single_v() {
        assert_eq!(normalise("v2.0.1"), "2.0.1");
        assert_eq!(normalise("2.0.1"), "2.0.1");
        assert_eq!(normalise("v2.0.2-rc.1"), "2.0.2-rc.1");
    }

    #[test]
    fn assert_all_match_reports_mismatches() {
        let ok = [Found {
            label: "Cargo.toml",
            version: "2.0.1".into(),
        }];
        assert!(assert_all_match("2.0.1", &ok).is_ok());

        let bad = [
            Found {
                label: "Cargo.toml",
                version: "2.0.1".into(),
            },
            Found {
                label: "package.json",
                version: "2.0.0".into(),
            },
        ];
        let err = assert_all_match("2.0.1", &bad).unwrap_err().to_string();
        assert!(err.contains("package.json = 2.0.0"));
        assert!(err.contains("cargo xtask bump 2.0.1"));
        assert!(!err.contains("Cargo.toml = 2.0.1 (expected"));
    }
}
