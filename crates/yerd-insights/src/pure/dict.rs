//! Per-site dictionary of route labels, SQL text and issue descriptors.
//!
//! Rollups reference these by `u64` key only. On disk each entry is written
//! into an hour's `.roll` file the first time its key appears in that hour, so
//! pruning an hour ages its entries out and a load rebuilds the dictionary
//! from the retained files. Written-key sets are kept for the current and
//! previous UTC hour only.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::pure::issues::IssueKind;
use crate::pure::route::RouteKind;

/// Most bytes of sample SQL kept per fingerprint.
pub const SAMPLE_SQL_MAX: usize = 2048;

/// One dictionary entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum DictEntry {
    /// A route.
    Route {
        /// HTTP method.
        method: String,
        /// Display label.
        label: String,
        /// How it was identified.
        kind: RouteKind,
        /// Framework route name.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// Framework action.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        action: Option<String>,
    },
    /// A query fingerprint.
    Sql {
        /// The normalised SQL.
        fingerprint: String,
        /// One concrete example, at most [`SAMPLE_SQL_MAX`] bytes.
        sample_sql: String,
    },
    /// An issue descriptor.
    Issue {
        /// Issue type.
        kind: IssueKind,
        /// Fingerprint hash, `None` for too-many-queries.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fp_hash: Option<u64>,
        /// Call site file.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file: Option<String>,
        /// Call site line.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        line: Option<u32>,
        /// The route it was first seen on.
        route_id: u64,
    },
}

/// Whether `new` should replace `old` under the same key: a route seen only as
/// a plain path is replaced once a request identifies it as a framework
/// template or script. The rule never downgrades, so loading hour files in any
/// order ends on the same entry.
fn upgrades(old: &DictEntry, new: &DictEntry) -> bool {
    matches!(
        (old, new),
        (
            DictEntry::Route {
                kind: RouteKind::Path,
                ..
            },
            DictEntry::Route { kind, .. },
        ) if *kind != RouteKind::Path
    )
}

/// The per-site dictionary.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Dict {
    entries: BTreeMap<u64, DictEntry>,
    written: Vec<(u64, BTreeSet<u64>)>,
}

impl Dict {
    /// An empty dictionary.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add `entry` under `key` if the key is absent, or if it upgrades the
    /// stored entry (a path-only route identified later as a template or
    /// script). An upgraded key is marked unwritten for every tracked hour so
    /// the next flush writes the new entry. Returns whether it was stored.
    pub fn insert(&mut self, key: u64, entry: DictEntry) -> bool {
        if let Some(old) = self.entries.get(&key) {
            if !upgrades(old, &entry) {
                return false;
            }
            for (_, set) in &mut self.written {
                set.remove(&key);
            }
        }
        self.entries.insert(key, entry);
        true
    }

    /// The entry for `key`.
    #[must_use]
    pub fn get(&self, key: u64) -> Option<&DictEntry> {
        self.entries.get(&key)
    }

    /// Whether `key` has an entry.
    #[must_use]
    pub fn contains(&self, key: u64) -> bool {
        self.entries.contains_key(&key)
    }

    /// Number of entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when there are no entries and no written-key sets.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.written.is_empty()
    }

    fn written_set(&mut self, hour: u64) -> Option<&mut BTreeSet<u64>> {
        let newest = self.written.iter().map(|(h, _)| *h).max();
        if !self.written.iter().any(|(h, _)| *h == hour) {
            if newest.is_some_and(|n| hour + 1 < n) {
                return None;
            }
            self.written.push((hour, BTreeSet::new()));
            self.written.sort_by_key(|(h, _)| *h);
            let newest = self.written.iter().map(|(h, _)| *h).max().unwrap_or(hour);
            self.written.retain(|(h, _)| h + 1 >= newest);
        }
        self.written
            .iter_mut()
            .find(|(h, _)| *h == hour)
            .map(|(_, s)| s)
    }

    /// The entries among `keys` not yet written for `hour`, marking them
    /// written. An hour older than the previous one is never tracked, so all
    /// its present entries are returned every time.
    pub fn take_unwritten(
        &mut self,
        hour: u64,
        keys: impl IntoIterator<Item = u64>,
    ) -> Vec<(u64, DictEntry)> {
        let wanted: BTreeSet<u64> = keys
            .into_iter()
            .filter(|k| self.entries.contains_key(k))
            .collect();
        let fresh: Vec<u64> = match self.written_set(hour) {
            Some(set) => {
                let fresh: Vec<u64> = wanted.difference(set).copied().collect();
                set.extend(fresh.iter().copied());
                fresh
            }
            None => wanted.into_iter().collect(),
        };
        fresh
            .into_iter()
            .filter_map(|k| self.entries.get(&k).map(|e| (k, e.clone())))
            .collect()
    }

    /// Mark `keys` as already written for `hour` (used when loading from
    /// disk).
    pub fn mark_written(&mut self, hour: u64, keys: impl IntoIterator<Item = u64>) {
        if let Some(set) = self.written_set(hour) {
            set.extend(keys);
        }
    }

    /// Forget which keys were written for `hour`, so the next flush for it
    /// rewrites every entry it references.
    pub fn forget_hour(&mut self, hour: u64) {
        self.written.retain(|(h, _)| *h != hour);
    }

    /// Hours with a written-key set, oldest first.
    #[must_use]
    pub fn tracked_hours(&self) -> Vec<u64> {
        self.written.iter().map(|(h, _)| *h).collect()
    }

    /// Keep only the `referenced` keys.
    pub fn retain(&mut self, referenced: &BTreeSet<u64>) {
        self.entries.retain(|k, _| referenced.contains(k));
    }

    /// Every entry, by key.
    pub fn iter(&self) -> impl Iterator<Item = (u64, &DictEntry)> {
        self.entries.iter().map(|(k, e)| (*k, e))
    }

    /// Add the keys an [`DictEntry::Issue`] under `key` refers to.
    pub fn extend_issue_refs(&self, keys: &mut BTreeSet<u64>) {
        let issue_refs: Vec<u64> = keys
            .iter()
            .filter_map(|k| match self.entries.get(k) {
                Some(DictEntry::Issue {
                    fp_hash, route_id, ..
                }) => Some([*fp_hash, Some(*route_id)]),
                _ => None,
            })
            .flatten()
            .flatten()
            .collect();
        keys.extend(issue_refs);
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

    fn sql(s: &str) -> DictEntry {
        DictEntry::Sql {
            fingerprint: s.into(),
            sample_sql: s.into(),
        }
    }

    #[test]
    fn insert_keeps_the_first_entry() {
        let mut d = Dict::new();
        assert!(d.insert(1, sql("a")));
        assert!(!d.insert(1, sql("b")));
        assert_eq!(d.get(1), Some(&sql("a")));
    }

    fn route(kind: RouteKind, name: Option<&str>) -> DictEntry {
        DictEntry::Route {
            method: "GET".into(),
            label: "/about".into(),
            kind,
            name: name.map(str::to_owned),
            action: None,
        }
    }

    #[test]
    fn route_descriptors_upgrade_from_path_only() {
        let path = route(RouteKind::Path, None);
        let template = route(RouteKind::Template, Some("about"));
        let script = route(RouteKind::Script, None);
        let cases: &[(&DictEntry, &DictEntry, bool)] = &[
            (&path, &template, true),
            (&path, &script, true),
            (&path, &path, false),
            (&template, &path, false),
            (&template, &script, false),
            (&script, &template, false),
            (&sql("a"), &template, false),
            (&path, &sql("a"), false),
        ];
        for (old, new, want) in cases {
            let mut d = Dict::new();
            d.insert(1, (*old).clone());
            assert!(d.take_unwritten(10, [1]).len() == 1);
            assert_eq!(d.insert(1, (*new).clone()), *want, "{old:?} -> {new:?}");
            let stored = if *want { *new } else { *old };
            assert_eq!(d.get(1), Some(stored));
            let rewritten = d.take_unwritten(10, [1]);
            assert_eq!(rewritten.len(), usize::from(*want), "{old:?} -> {new:?}");
        }
    }

    #[test]
    fn take_unwritten_per_hour() {
        let mut d = Dict::new();
        d.insert(1, sql("a"));
        d.insert(2, sql("b"));
        assert_eq!(d.take_unwritten(10, [1, 2, 3]).len(), 2);
        assert!(d.take_unwritten(10, [1, 2]).is_empty());
        assert_eq!(d.take_unwritten(11, [1]).len(), 1);
        d.forget_hour(10);
        assert_eq!(d.take_unwritten(10, [1, 2]).len(), 2);
        assert_eq!(d.tracked_hours(), vec![10, 11]);
        assert_eq!(d.take_unwritten(12, [2]).len(), 1);
        assert_eq!(d.tracked_hours(), vec![11, 12]);
        assert_eq!(d.take_unwritten(10, [1]).len(), 1);
        assert_eq!(d.take_unwritten(10, [1]).len(), 1);
        assert_eq!(d.tracked_hours(), vec![11, 12]);
    }

    #[test]
    fn retain_drops_unreferenced_keys() {
        let mut d = Dict::new();
        d.insert(1, sql("a"));
        d.insert(2, sql("b"));
        d.insert(3, sql("c"));
        d.retain(&BTreeSet::from([1, 3]));
        assert!(d.contains(1) && !d.contains(2) && d.contains(3));
        assert_eq!(d.len(), 2);
    }

    #[test]
    fn issue_refs_extend_the_closure() {
        let mut d = Dict::new();
        d.insert(
            9,
            DictEntry::Issue {
                kind: IssueKind::NPlusOne,
                fp_hash: Some(4),
                file: None,
                line: None,
                route_id: 5,
            },
        );
        let mut keys = BTreeSet::from([9]);
        d.extend_issue_refs(&mut keys);
        assert_eq!(keys, BTreeSet::from([4, 5, 9]));
    }
}
