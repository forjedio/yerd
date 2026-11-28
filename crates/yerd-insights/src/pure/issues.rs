//! Per-request issue detection over captured queries.
//!
//! Rules (thresholds in [`Thresholds`]):
//!
//! * **N+1:** queries grouped by `(fp_hash, file, line)`; a group with at least
//!   `n_plus_one_min` members and at least two distinct concrete values. This
//!   catches bound parameters and inline literals alike.
//! * **Duplicate:** queries grouped by `raw_key` (identical SQL and bindings);
//!   at least `duplicate_min` members. Members already reported as N+1 are not
//!   reported again.
//! * **Slow:** a query over `slow_query_ms`.
//! * **Large read:** a read without `LIMIT` returning at least
//!   `large_read_rows` rows.
//! * **Too many queries:** more than `max_queries` in the request.
//!
//! Hits sharing an issue key (`kind`, fingerprint, call site) are merged.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::pure::sql::{fingerprint, fnv, has_limit, is_read};

/// Issue-detection thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Thresholds {
    /// Minimum repeats of one query shape from one call site to flag an N+1.
    pub n_plus_one_min: u32,
    /// Minimum identical queries to flag a duplicate.
    pub duplicate_min: u32,
    /// A query slower than this many milliseconds is slow.
    pub slow_query_ms: u32,
    /// A read without `LIMIT` returning at least this many rows is large.
    pub large_read_rows: u32,
    /// A request running more than this many queries is flagged.
    pub max_queries: u32,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            n_plus_one_min: 5,
            duplicate_min: 2,
            slow_query_ms: 100,
            large_read_rows: 1000,
            max_queries: 50,
        }
    }
}

/// A detected issue type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueKind {
    /// Repeated query shape from one call site.
    NPlusOne,
    /// Identical query run more than once.
    Duplicate,
    /// Slow query.
    SlowQuery,
    /// Large unbounded read.
    LargeRead,
    /// Too many queries in one request.
    TooManyQueries,
}

impl IssueKind {
    /// Every kind, in display order.
    pub const ALL: [Self; 5] = [
        Self::NPlusOne,
        Self::Duplicate,
        Self::SlowQuery,
        Self::LargeRead,
        Self::TooManyQueries,
    ];

    /// Stable short name, also used in issue keys.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NPlusOne => "n_plus_one",
            Self::Duplicate => "duplicate",
            Self::SlowQuery => "slow_query",
            Self::LargeRead => "large_read",
            Self::TooManyQueries => "too_many_queries",
        }
    }

    /// A one-bit mask for compact kind sets.
    #[must_use]
    pub fn bit(self) -> u8 {
        match self {
            Self::NPlusOne => 1,
            Self::Duplicate => 2,
            Self::SlowQuery => 4,
            Self::LargeRead => 8,
            Self::TooManyQueries => 16,
        }
    }

    /// The kinds present in a bit mask, in display order.
    #[must_use]
    pub fn from_bits(bits: u8) -> Vec<Self> {
        Self::ALL
            .into_iter()
            .filter(|k| bits & k.bit() != 0)
            .collect()
    }

    /// The wire equivalent.
    #[must_use]
    pub fn to_ipc(self) -> yerd_ipc::InsightsIssueKind {
        match self {
            Self::NPlusOne => yerd_ipc::InsightsIssueKind::NPlusOne,
            Self::Duplicate => yerd_ipc::InsightsIssueKind::Duplicate,
            Self::SlowQuery => yerd_ipc::InsightsIssueKind::SlowQuery,
            Self::LargeRead => yerd_ipc::InsightsIssueKind::LargeRead,
            Self::TooManyQueries => yerd_ipc::InsightsIssueKind::TooManyQueries,
        }
    }
}

/// A short, static fix hint for an issue type.
#[must_use]
pub fn hint(kind: IssueKind) -> &'static str {
    match kind {
        IssueKind::NPlusOne => {
            "The same query runs once per item from one place. Eager load the relation (with()) or batch the lookups."
        }
        IssueKind::Duplicate => {
            "The identical query runs more than once in this request. Reuse the first result or cache it."
        }
        IssueKind::SlowQuery => {
            "This query is slow. Check its indexes and how many rows it scans."
        }
        IssueKind::LargeRead => {
            "This read returns many rows without a LIMIT. Paginate, chunk or select fewer rows."
        }
        IssueKind::TooManyQueries => {
            "This request runs a lot of queries. Look for loops that query and for N+1 patterns."
        }
    }
}

/// Stable issue key: FNV-1a over `kind|fp_hash|file|line`.
#[must_use]
pub fn issue_key(kind: IssueKind, fp_hash: u64, file: Option<&str>, line: Option<u32>) -> u64 {
    let s = format!(
        "{}|{fp_hash:016x}|{}|{}",
        kind.as_str(),
        file.unwrap_or(""),
        line.map(|l| l.to_string()).unwrap_or_default()
    );
    fnv(s.as_bytes())
}

/// One captured query, reduced to what detection needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryEvent {
    /// The event's index in the request's timeline.
    pub idx: u32,
    /// Hash of the SQL fingerprint.
    pub fp_hash: u64,
    /// Hash of the exact SQL text plus canonical bindings.
    pub raw_key: u64,
    /// Hash of the concrete values: `sql_full` when present, else `raw_key`.
    pub value_key: u64,
    /// Query time in microseconds.
    pub time_us: u64,
    /// Rows returned, when known.
    pub rows: Option<u64>,
    /// Application call site file.
    pub file: Option<String>,
    /// Application call site line.
    pub line: Option<u32>,
    /// Whether the statement reads.
    pub is_read: bool,
    /// Whether the fingerprint has a `LIMIT`.
    pub has_limit: bool,
}

impl QueryEvent {
    /// Build from a query's parts. `fp` is `fingerprint(sql)`, passed in so
    /// the caller can reuse it.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        idx: u32,
        sql: &str,
        fp: &str,
        sql_full: &str,
        bindings: &Value,
        time_us: u64,
        rows: Option<u64>,
        file: Option<String>,
        line: Option<u32>,
    ) -> Self {
        let mut raw = String::with_capacity(sql.len() + 16);
        raw.push_str(sql);
        raw.push('\0');
        raw.push_str(&bindings.to_string());
        let raw_key = fnv(raw.as_bytes());
        let value_key = if sql_full.is_empty() {
            raw_key
        } else {
            fnv(sql_full.as_bytes())
        };
        Self {
            idx,
            fp_hash: fnv(fp.as_bytes()),
            raw_key,
            value_key,
            time_us,
            rows,
            file,
            line,
            is_read: is_read(sql),
            has_limit: has_limit(fp),
        }
    }

    /// Build from SQL only, fingerprinting it here.
    #[must_use]
    pub fn from_sql(idx: u32, sql: &str, bindings: &Value, time_us: u64) -> Self {
        Self::new(
            idx,
            sql,
            &fingerprint(sql),
            "",
            bindings,
            time_us,
            None,
            None,
            None,
        )
    }
}

/// One detected issue within a request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueHit {
    /// Issue type.
    pub kind: IssueKind,
    /// Stable issue key (see [`issue_key`]).
    pub key: u64,
    /// The timeline indices involved.
    pub event_indices: Vec<u32>,
    /// How many queries are involved (for too-many, the query count).
    pub count: u32,
    /// Their total time in microseconds.
    pub time_us: u64,
    /// Fingerprint hash, `None` for too-many-queries.
    pub fp_hash: Option<u64>,
    /// Call site file.
    pub file: Option<String>,
    /// Call site line.
    pub line: Option<u32>,
}

fn add_hit(hits: &mut BTreeMap<u64, IssueHit>, kind: IssueKind, members: &[&QueryEvent]) {
    let Some(first) = members.first() else {
        return;
    };
    let key = issue_key(kind, first.fp_hash, first.file.as_deref(), first.line);
    let hit = hits.entry(key).or_insert_with(|| IssueHit {
        kind,
        key,
        event_indices: Vec::new(),
        count: 0,
        time_us: 0,
        fp_hash: Some(first.fp_hash),
        file: first.file.clone(),
        line: first.line,
    });
    for m in members {
        hit.event_indices.push(m.idx);
        hit.count = hit.count.saturating_add(1);
        hit.time_us = hit.time_us.saturating_add(m.time_us);
    }
}

/// A query's call site: fingerprint hash, file and line.
type CallSite<'a> = (u64, Option<&'a str>, Option<u32>);

/// Detect issues in one request's queries.
#[must_use]
pub fn detect(events: &[QueryEvent], t: &Thresholds) -> Vec<IssueHit> {
    let mut hits: BTreeMap<u64, IssueHit> = BTreeMap::new();
    let mut by_site: BTreeMap<CallSite<'_>, Vec<&QueryEvent>> = BTreeMap::new();
    for e in events {
        by_site
            .entry((e.fp_hash, e.file.as_deref(), e.line))
            .or_default()
            .push(e);
    }
    let mut in_n_plus_one: BTreeSet<u32> = BTreeSet::new();
    for members in by_site.values() {
        let distinct: BTreeSet<u64> = members.iter().map(|m| m.value_key).collect();
        if members.len() >= t.n_plus_one_min as usize && distinct.len() >= 2 {
            in_n_plus_one.extend(members.iter().map(|m| m.idx));
            add_hit(&mut hits, IssueKind::NPlusOne, members);
        }
    }
    let mut by_raw: BTreeMap<u64, Vec<&QueryEvent>> = BTreeMap::new();
    for e in events.iter().filter(|e| !in_n_plus_one.contains(&e.idx)) {
        by_raw.entry(e.raw_key).or_default().push(e);
    }
    for members in by_raw.values() {
        if members.len() >= t.duplicate_min.max(2) as usize {
            add_hit(&mut hits, IssueKind::Duplicate, members);
        }
    }
    let slow_us = u64::from(t.slow_query_ms).saturating_mul(1000);
    for e in events.iter().filter(|e| e.time_us > slow_us) {
        add_hit(&mut hits, IssueKind::SlowQuery, &[e]);
    }
    for e in events.iter().filter(|e| {
        e.is_read && !e.has_limit && e.rows.is_some_and(|r| r >= u64::from(t.large_read_rows))
    }) {
        add_hit(&mut hits, IssueKind::LargeRead, &[e]);
    }
    if events.len() > t.max_queries as usize {
        let key = issue_key(IssueKind::TooManyQueries, 0, None, None);
        hits.insert(
            key,
            IssueHit {
                kind: IssueKind::TooManyQueries,
                key,
                event_indices: Vec::new(),
                count: u32::try_from(events.len()).unwrap_or(u32::MAX),
                time_us: events.iter().map(|e| e.time_us).sum(),
                fp_hash: None,
                file: None,
                line: None,
            },
        );
    }
    let mut out: Vec<IssueHit> = hits.into_values().collect();
    out.sort_by_key(|h| (h.kind, h.event_indices.first().copied(), h.key));
    out
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::type_complexity
)]
mod tests {
    use serde_json::json;

    use super::*;

    struct Q {
        sql: String,
        full: String,
        bindings: Value,
        us: u64,
        rows: Option<u64>,
        line: u32,
    }

    fn q(sql: &str, bindings: Value, line: u32) -> Q {
        Q {
            sql: sql.to_owned(),
            full: String::new(),
            bindings,
            us: 100,
            rows: None,
            line,
        }
    }

    fn events(qs: &[Q]) -> Vec<QueryEvent> {
        qs.iter()
            .enumerate()
            .map(|(i, x)| {
                QueryEvent::new(
                    i as u32,
                    &x.sql,
                    &fingerprint(&x.sql),
                    &x.full,
                    &x.bindings,
                    x.us,
                    x.rows,
                    Some("app/Http/C.php".into()),
                    Some(x.line),
                )
            })
            .collect()
    }

    fn kinds(hits: &[IssueHit]) -> Vec<(IssueKind, u32)> {
        hits.iter().map(|h| (h.kind, h.count)).collect()
    }

    #[test]
    fn detection_table() {
        let t = Thresholds::default();
        let bound = |n: usize, line: u32| -> Vec<Q> {
            (0..n)
                .map(|i| q("select * from users where id = ?", json!([i]), line))
                .collect()
        };
        let inline: Vec<Q> = (1..=5)
            .map(|i| {
                q(
                    &format!("select * from users where id = {i}"),
                    json!([]),
                    10,
                )
            })
            .collect();
        let inline_dup = vec![
            q("select * from users where id = 7", json!([]), 10),
            q("select * from users where id = 7", json!([]), 10),
        ];
        let same_bindings = vec![
            q("select * from users where id = ?", json!([1]), 10),
            q("select * from users where id = ?", json!([1]), 10),
        ];
        let mut two_sites = bound(5, 10);
        two_sites.extend(bound(5, 20));
        let mut limited = q("select * from users limit 5000", json!([]), 1);
        limited.rows = Some(5000);
        let mut unlimited = q("select * from users", json!([]), 1);
        unlimited.rows = Some(1000);
        let mut small = q("select * from users", json!([]), 1);
        small.rows = Some(999);
        let rows_missing = q("select * from users", json!([]), 1);
        let mut slow = q("select sleep(1)", json!([]), 1);
        slow.us = 100_001;
        let mut not_slow = q("select sleep(1)", json!([]), 1);
        not_slow.us = 100_000;

        let cases: Vec<(&str, Vec<Q>, Vec<(IssueKind, u32)>)> = vec![
            ("bound n+1", bound(5, 10), vec![(IssueKind::NPlusOne, 5)]),
            ("below n+1 min", bound(4, 10), vec![]),
            ("inline-literal n+1", inline, vec![(IssueKind::NPlusOne, 5)]),
            (
                "inline-literal duplicate",
                inline_dup,
                vec![(IssueKind::Duplicate, 2)],
            ),
            (
                "identical bindings",
                same_bindings,
                vec![(IssueKind::Duplicate, 2)],
            ),
            (
                "two call sites",
                two_sites,
                vec![(IssueKind::NPlusOne, 5), (IssueKind::NPlusOne, 5)],
            ),
            ("limit present", vec![limited], vec![]),
            (
                "large read",
                vec![unlimited],
                vec![(IssueKind::LargeRead, 1)],
            ),
            ("below large read", vec![small], vec![]),
            ("rows missing", vec![rows_missing], vec![]),
            ("slow", vec![slow], vec![(IssueKind::SlowQuery, 1)]),
            ("exactly the slow threshold", vec![not_slow], vec![]),
        ];
        for (name, qs, want) in cases {
            assert_eq!(kinds(&detect(&events(&qs), &t)), want, "{name}");
        }
    }

    #[test]
    fn n_plus_one_members_are_not_also_duplicates() {
        let mut qs: Vec<Q> = (0..5)
            .map(|i| q("select * from users where id = ?", json!([i]), 10))
            .collect();
        qs.push(q("select * from users where id = ?", json!([0]), 10));
        let hits = detect(&events(&qs), &Thresholds::default());
        assert_eq!(kinds(&hits), vec![(IssueKind::NPlusOne, 6)]);
        assert_eq!(hits[0].event_indices, vec![0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn sql_full_distinguishes_values() {
        let mut qs: Vec<Q> = (0..5)
            .map(|_| q("select * from users where id = ?", json!([]), 10))
            .collect();
        for (i, x) in qs.iter_mut().enumerate() {
            x.full = format!("select * from users where id = {i}");
        }
        let hits = detect(&events(&qs), &Thresholds::default());
        assert_eq!(kinds(&hits), vec![(IssueKind::NPlusOne, 5)]);
    }

    #[test]
    fn too_many_queries_boundary() {
        let t = Thresholds {
            max_queries: 3,
            n_plus_one_min: 100,
            ..Thresholds::default()
        };
        let mk = |n: usize| -> Vec<Q> {
            (0..n)
                .map(|i| q("select * from t where id = ?", json!([i]), 1))
                .collect()
        };
        assert_eq!(kinds(&detect(&events(&mk(3)), &t)), vec![]);
        assert_eq!(
            kinds(&detect(&events(&mk(4)), &t)),
            vec![(IssueKind::TooManyQueries, 4)]
        );
    }

    #[test]
    fn duplicate_min_boundary() {
        let t = Thresholds {
            duplicate_min: 3,
            ..Thresholds::default()
        };
        let mk = |n: usize| -> Vec<Q> { (0..n).map(|_| q("select 1", json!([]), 1)).collect() };
        assert_eq!(kinds(&detect(&events(&mk(2)), &t)), vec![]);
        assert_eq!(
            kinds(&detect(&events(&mk(3)), &t)),
            vec![(IssueKind::Duplicate, 3)]
        );
    }

    #[test]
    fn keys_are_stable_and_distinct() {
        let a = issue_key(IssueKind::NPlusOne, 1, Some("f.php"), Some(3));
        assert_eq!(a, fnv(b"n_plus_one|0000000000000001|f.php|3"));
        assert_ne!(
            a,
            issue_key(IssueKind::Duplicate, 1, Some("f.php"), Some(3))
        );
        assert_ne!(a, issue_key(IssueKind::NPlusOne, 1, Some("f.php"), Some(4)));
        for k in IssueKind::ALL {
            assert!(!hint(k).is_empty());
            assert_eq!(IssueKind::from_bits(k.bit()), vec![k]);
        }
    }
}
