//! Numeric Node release selectors shared by installation and execution.

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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
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
}
