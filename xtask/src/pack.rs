//! Pure, I/O-free helpers for assembling a Debian package.
//!
//! Everything here is deterministic and unit-tested; the actual staging and
//! `dpkg-deb` invocation lives in [`crate::deb`].

/// Metadata for a binary `.deb`'s `control` file.
pub struct DebMeta {
    /// `Package:` — the package name (e.g. `yerd`).
    pub package: String,
    /// `Version:` — the upstream version (e.g. `0.1.0`).
    pub version: String,
    /// `Architecture:` — a Debian arch (e.g. `amd64`).
    pub arch: String,
    /// `Maintainer:` — `Name <email>`.
    pub maintainer: String,
    /// `Section:` — the archive section (e.g. `devel`).
    pub section: String,
    /// `Priority:` — the package priority (e.g. `optional`).
    pub priority: String,
    /// `Depends:` — comma-separated dependency list (e.g. `libcap2-bin`).
    pub depends: String,
    /// First line is the synopsis; remaining lines form the extended
    /// description (each emitted as a leading-space continuation line).
    pub description: String,
}

/// Render a [`DebMeta`] as a Debian `control` stanza (trailing newline included).
///
/// The `Description` field is emitted as a one-line synopsis followed by
/// leading-space continuation lines for any further lines, per Debian policy.
#[must_use]
pub fn render_control(meta: &DebMeta) -> String {
    let mut desc_lines = meta.description.lines();
    let synopsis = desc_lines.next().unwrap_or("");
    let mut out = String::new();
    out.push_str(&format!("Package: {}\n", meta.package));
    out.push_str(&format!("Version: {}\n", meta.version));
    out.push_str(&format!("Section: {}\n", meta.section));
    out.push_str(&format!("Priority: {}\n", meta.priority));
    out.push_str(&format!("Architecture: {}\n", meta.arch));
    out.push_str(&format!("Depends: {}\n", meta.depends));
    out.push_str(&format!("Maintainer: {}\n", meta.maintainer));
    out.push_str(&format!("Description: {synopsis}\n"));
    for line in desc_lines {
        // A blank extended-description line is written as " ." per policy.
        if line.trim().is_empty() {
            out.push_str(" .\n");
        } else {
            out.push_str(&format!(" {line}\n"));
        }
    }
    out
}

/// Map a Rust target arch ([`std::env::consts::ARCH`]) to a Debian arch.
///
/// Returns `None` for arches this packaging path does not support yet.
#[must_use]
pub fn debian_arch(rust_arch: &str) -> Option<&'static str> {
    match rust_arch {
        "x86_64" => Some("amd64"),
        "aarch64" => Some("arm64"),
        _ => None,
    }
}

/// The conventional `.deb` filename: `{package}_{version}_{arch}.deb`.
#[must_use]
pub fn deb_filename(package: &str, version: &str, arch: &str) -> String {
    format!("{package}_{version}_{arch}.deb")
}

/// Extract a version from a `--version` line (e.g. `"yerd 0.1.0"` → `"0.1.0"`).
///
/// Returns the last whitespace-separated token of the first non-empty line, or
/// `None` if there is no such token.
#[must_use]
pub fn parse_version(version_output: &str) -> Option<String> {
    version_output
        .lines()
        .find(|l| !l.trim().is_empty())?
        .split_whitespace()
        .next_back()
        .map(ToOwned::to_owned)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    fn sample() -> DebMeta {
        DebMeta {
            package: "yerd".into(),
            version: "0.1.0".into(),
            arch: "amd64".into(),
            maintainer: "Maint <m@example.com>".into(),
            section: "devel".into(),
            priority: "optional".into(),
            depends: "libcap2-bin".into(),
            description: "One-line synopsis\nExtended line one.\nExtended line two.".into(),
        }
    }

    #[test]
    fn control_has_required_fields_and_values() {
        let c = render_control(&sample());
        assert!(c.contains("Package: yerd\n"));
        assert!(c.contains("Version: 0.1.0\n"));
        assert!(c.contains("Architecture: amd64\n"));
        assert!(c.contains("Depends: libcap2-bin\n"));
        assert!(c.contains("Maintainer: Maint <m@example.com>\n"));
        assert!(c.ends_with('\n'));
    }

    #[test]
    fn control_description_uses_continuation_lines() {
        let c = render_control(&sample());
        assert!(c.contains("Description: One-line synopsis\n"));
        assert!(c.contains("\n Extended line one.\n"));
        assert!(c.contains("\n Extended line two.\n"));
    }

    #[test]
    fn control_blank_extended_line_becomes_dot() {
        let mut m = sample();
        m.description = "Synopsis\n\nAfter a blank.".into();
        let c = render_control(&m);
        assert!(c.contains("\n .\n"));
        assert!(c.contains("\n After a blank.\n"));
    }

    #[test]
    fn debian_arch_maps_known_and_rejects_unknown() {
        assert_eq!(debian_arch("x86_64"), Some("amd64"));
        assert_eq!(debian_arch("aarch64"), Some("arm64"));
        assert_eq!(debian_arch("riscv64"), None);
    }

    #[test]
    fn deb_filename_shape() {
        assert_eq!(
            deb_filename("yerd", "0.1.0", "amd64"),
            "yerd_0.1.0_amd64.deb"
        );
    }

    #[test]
    fn parse_version_extracts_last_token() {
        assert_eq!(parse_version("yerd 0.1.0\n").as_deref(), Some("0.1.0"));
        assert_eq!(parse_version("yerd 1.2.3").as_deref(), Some("1.2.3"));
    }

    #[test]
    fn parse_version_rejects_empty() {
        assert_eq!(parse_version(""), None);
        assert_eq!(parse_version("   \n  \n"), None);
    }
}
