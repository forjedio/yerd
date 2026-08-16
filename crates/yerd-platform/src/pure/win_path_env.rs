//! Pure editor for a Windows `;`-separated PATH-style list.
//!
//! Idempotent add/remove of directory entries in the `HKCU\Environment\Path`
//! value, comparing case-insensitively and ignoring a trailing slash (the two
//! ways the same directory renders). Order-preserving and append-only on insert;
//! returns `None` when the edit would be a no-op so the caller can skip the
//! registry write (and the `WM_SETTINGCHANGE` broadcast).
//!
//! Un-gated (compiled and table-tested on every OS): pure string manipulation.

/// Normalised comparison key for a single entry: trimmed, trailing `\`/`/`
/// stripped, ASCII-lowercased. Empty after trimming means "no entry".
fn norm(entry: &str) -> String {
    entry
        .trim()
        .trim_end_matches(['\\', '/'])
        .to_ascii_lowercase()
}

/// Append each of `add` to the `;`-list `current` that isn't already present,
/// preserving existing order and formatting. Returns `Some(new_value)` when at
/// least one entry was appended, or `None` when every entry was already present
/// (an idempotent no-op).
#[must_use]
pub fn upsert_entries(current: &str, add: &[&str]) -> Option<String> {
    let present: Vec<String> = current
        .split(';')
        .filter(|s| !s.trim().is_empty())
        .map(norm)
        .collect();

    let mut result = current.to_owned();
    let mut appended = false;
    for &entry in add {
        let key = norm(entry);
        if key.is_empty() {
            continue;
        }
        if present.contains(&key) || (appended && norm_list_contains(&result, &key)) {
            continue;
        }
        if !result.is_empty() && !result.ends_with(';') {
            result.push(';');
        }
        result.push_str(entry);
        appended = true;
    }
    appended.then_some(result)
}

/// Remove every entry of `current` matching any of `remove` (by normalised key),
/// preserving the order and any empty segments of the rest. Returns
/// `Some(new_value)` when at least one entry was removed, else `None`.
#[must_use]
pub fn remove_entries(current: &str, remove: &[&str]) -> Option<String> {
    let targets: Vec<String> = remove
        .iter()
        .map(|s| norm(s))
        .filter(|s| !s.is_empty())
        .collect();
    if targets.is_empty() {
        return None;
    }

    let mut changed = false;
    let kept: Vec<&str> = current
        .split(';')
        .filter(|seg| {
            if seg.trim().is_empty() {
                return true;
            }
            if targets.contains(&norm(seg)) {
                changed = true;
                false
            } else {
                true
            }
        })
        .collect();

    changed.then(|| kept.join(";"))
}

/// Whether the `;`-list `current` holds `entry`, compared by normalised key.
/// Lets a caller report exactly which entries a [`remove_entries`] edit took
/// out, instead of every entry it was asked to look for.
#[must_use]
pub fn contains_entry(current: &str, entry: &str) -> bool {
    let key = norm(entry);
    !key.is_empty() && norm_list_contains(current, &key)
}

/// Whether `list` (a `;`-value) already holds `key` (a normalised entry). Used
/// to dedup within a single multi-entry `upsert` call.
fn norm_list_contains(list: &str, key: &str) -> bool {
    list.split(';')
        .filter(|s| !s.trim().is_empty())
        .any(|s| norm(s) == key)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn upsert_into_empty_value() {
        assert_eq!(
            upsert_entries("", &[r"C:\yerd\bin"]).as_deref(),
            Some(r"C:\yerd\bin")
        );
    }

    #[test]
    fn upsert_appends_without_trailing_semicolon() {
        assert_eq!(
            upsert_entries(r"C:\Windows;C:\Windows\System32", &[r"C:\yerd\bin"]).as_deref(),
            Some(r"C:\Windows;C:\Windows\System32;C:\yerd\bin")
        );
    }

    #[test]
    fn upsert_preserves_existing_trailing_semicolon() {
        assert_eq!(
            upsert_entries(r"C:\Windows;", &[r"C:\yerd\bin"]).as_deref(),
            Some(r"C:\Windows;C:\yerd\bin")
        );
    }

    #[test]
    fn upsert_is_noop_when_present() {
        assert_eq!(upsert_entries(r"C:\yerd\bin", &[r"C:\yerd\bin"]), None);
    }

    #[test]
    fn upsert_matches_case_insensitively_and_ignores_trailing_slash() {
        assert_eq!(upsert_entries(r"c:\yerd\BIN\", &[r"C:\yerd\bin"]), None);
    }

    #[test]
    fn upsert_adds_only_the_missing_of_several() {
        assert_eq!(
            upsert_entries(r"C:\yerd\bin", &[r"C:\yerd\bin", r"C:\yerd\shim"]).as_deref(),
            Some(r"C:\yerd\bin;C:\yerd\shim")
        );
    }

    #[test]
    fn upsert_dedups_repeated_add_entries() {
        assert_eq!(
            upsert_entries("", &[r"C:\yerd\bin", r"C:\yerd\bin\"]).as_deref(),
            Some(r"C:\yerd\bin")
        );
    }

    #[test]
    fn upsert_on_a_long_preexisting_path() {
        let long =
            r"C:\Windows;C:\Windows\System32;C:\Program Files\Git\cmd;C:\Users\me\.cargo\bin";
        let out = upsert_entries(long, &[r"C:\yerd\bin"]).unwrap();
        assert!(out.starts_with(long));
        assert!(out.ends_with(r";C:\yerd\bin"));
    }

    #[test]
    fn remove_drops_matching_entry() {
        assert_eq!(
            remove_entries(r"C:\Windows;C:\yerd\bin;C:\other", &[r"C:\yerd\bin"]).as_deref(),
            Some(r"C:\Windows;C:\other")
        );
    }

    #[test]
    fn remove_is_noop_when_absent() {
        assert_eq!(remove_entries(r"C:\Windows", &[r"C:\yerd\bin"]), None);
    }

    #[test]
    fn remove_matches_case_and_trailing_slash() {
        assert_eq!(
            remove_entries(r"C:\Windows;C:\YERD\Bin\", &[r"C:\yerd\bin"]).as_deref(),
            Some(r"C:\Windows")
        );
    }

    #[test]
    fn remove_several_entries() {
        assert_eq!(
            remove_entries(
                r"C:\a;C:\yerd\bin;C:\b;C:\yerd\shim",
                &[r"C:\yerd\bin", r"C:\yerd\shim"]
            )
            .as_deref(),
            Some(r"C:\a;C:\b")
        );
    }

    #[test]
    fn remove_of_empty_target_list_is_noop() {
        assert_eq!(remove_entries(r"C:\a", &[]), None);
        assert_eq!(remove_entries(r"C:\a", &["  "]), None);
    }

    #[test]
    fn contains_entry_matches_the_same_key_as_remove() {
        let current = r"C:\Windows;c:\yerd\BIN\";
        let cases: &[(&str, bool)] = &[
            (r"C:\yerd\bin", true),
            (r"C:\yerd\bin\", true),
            (r"C:\Windows", true),
            (r"C:\yerd\shim", false),
            ("", false),
            ("   ", false),
        ];
        for (entry, want) in cases {
            assert_eq!(contains_entry(current, entry), *want, "entry: {entry}");
        }
    }
}
