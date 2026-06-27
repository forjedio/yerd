//! Parse Firefox `profiles.ini` to extract profile directory entries.
//!
//! Output is intentionally raw - callers join `Path` against the parent
//! directory of `profiles.ini` (which `pure::firefox` does not know about)
//! when `is_relative` is true.

/// A single profile entry from `profiles.ini`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    /// `Name=` field of the profile section.
    pub name: String,
    /// `Path=` field; either relative (against the profiles.ini parent) or
    /// absolute, controlled by [`Self::is_relative`].
    pub path: String,
    /// `IsRelative=1` if present (default `false`).
    pub is_relative: bool,
    /// `Default=1` if present (default `false`).
    pub is_default: bool,
}

/// Parse a `profiles.ini` text and return every `[Profile<N>]` section.
///
/// Unknown keys, comments, and sections other than `Profile<N>` are
/// silently ignored. Profiles without a `Path=` line are skipped.
#[must_use]
pub fn parse_profiles_ini(text: &str) -> Vec<Profile> {
    let mut out = Vec::new();
    let mut current: Option<ProfileBuilder> = None;

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if let Some(section) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            open_section(section, &mut current, &mut out);
            continue;
        }
        let Some(b) = current.as_mut() else { continue };
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        b.set(key.trim(), value.trim());
    }
    push_built(current, &mut out);
    out
}

/// On a `[section]` line: flush the in-progress profile (if any), then begin a
/// fresh builder iff this is a `Profile<N>` section.
fn open_section(section: &str, current: &mut Option<ProfileBuilder>, out: &mut Vec<Profile>) {
    push_built(current.take(), out);
    *current = is_profile_section(section).then(ProfileBuilder::default);
}

/// Build `builder` (if any) and push the result to `out` when it yields a profile
/// (i.e. it had a `Path=`).
fn push_built(builder: Option<ProfileBuilder>, out: &mut Vec<Profile>) {
    if let Some(b) = builder {
        if let Some(p) = b.build() {
            out.push(p);
        }
    }
}

fn is_profile_section(name: &str) -> bool {
    name.strip_prefix("Profile")
        .is_some_and(|tail| !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()))
}

#[derive(Default)]
struct ProfileBuilder {
    name: Option<String>,
    path: Option<String>,
    is_relative: bool,
    is_default: bool,
}

impl ProfileBuilder {
    /// Apply one `Key=Value` line (unknown keys are ignored).
    fn set(&mut self, key: &str, value: &str) {
        match key {
            "Name" => self.name = Some(value.to_owned()),
            "Path" => self.path = Some(value.to_owned()),
            "IsRelative" => self.is_relative = value == "1",
            "Default" => self.is_default = value == "1",
            _ => {}
        }
    }

    fn build(self) -> Option<Profile> {
        let path = self.path?;
        Some(Profile {
            name: self.name.unwrap_or_default(),
            path,
            is_relative: self.is_relative,
            is_default: self.is_default,
        })
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

    #[test]
    fn parses_minimal_relative_profile() {
        let text = "[Profile0]\nName=default\nIsRelative=1\nPath=abc.default\n";
        let profiles = parse_profiles_ini(text);
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].name, "default");
        assert_eq!(profiles[0].path, "abc.default");
        assert!(profiles[0].is_relative);
        assert!(!profiles[0].is_default);
    }

    #[test]
    fn parses_multiple_profiles() {
        let text = "\
[General]
StartWithLastProfile=1
Version=2

[Profile0]
Name=default
IsRelative=1
Path=p0.default

[Profile1]
Name=other
IsRelative=0
Path=/abs/path
Default=1
";
        let profiles = parse_profiles_ini(text);
        assert_eq!(profiles.len(), 2);
        assert_eq!(profiles[0].name, "default");
        assert_eq!(profiles[0].path, "p0.default");
        assert!(profiles[0].is_relative);
        assert!(!profiles[0].is_default);
        assert_eq!(profiles[1].name, "other");
        assert_eq!(profiles[1].path, "/abs/path");
        assert!(!profiles[1].is_relative);
        assert!(profiles[1].is_default);
    }

    #[test]
    fn ignores_install_and_general_sections() {
        let text = "\
[Install4F96D1932A9F858E]
Default=p0.default

[General]
Version=2

[Profile0]
Name=default
Path=p0.default
";
        let profiles = parse_profiles_ini(text);
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].name, "default");
    }

    #[test]
    fn skips_profile_without_path() {
        let text = "[Profile0]\nName=broken\nIsRelative=1\n";
        assert!(parse_profiles_ini(text).is_empty());
    }

    #[test]
    fn ignores_comments_and_blank_lines() {
        let text = "; comment\n# another\n\n[Profile0]\nPath=x\n";
        let p = parse_profiles_ini(text);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].path, "x");
    }

    #[test]
    fn is_relative_defaults_false_when_missing() {
        let text = "[Profile0]\nPath=abs.default\n";
        let p = parse_profiles_ini(text);
        assert!(!p[0].is_relative);
    }

    #[test]
    fn handles_trailing_whitespace_in_keys_and_values() {
        let text = "[Profile0]\n  Name = foo  \n  Path = bar  \n";
        let p = parse_profiles_ini(text);
        assert_eq!(p[0].name, "foo");
        assert_eq!(p[0].path, "bar");
    }

    #[test]
    fn empty_input_yields_empty_vec() {
        assert!(parse_profiles_ini("").is_empty());
    }

    #[test]
    fn section_name_profilenondigit_is_ignored() {
        let text = "[ProfileX]\nPath=x\n";
        assert!(parse_profiles_ini(text).is_empty());
    }

    #[test]
    fn last_profile_without_trailing_newline_still_parses() {
        let text = "[Profile0]\nPath=last";
        let p = parse_profiles_ini(text);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].path, "last");
    }
}
