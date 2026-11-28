//! Finalised request records: the list row ([`FinalRequest`]) and the
//! drill-in timeline ([`RequestDetail`]), plus their wire conversions.

use serde::{Deserialize, Serialize};
use yerd_ipc::DumpCategory;

use crate::pure::issues::{hint, IssueHit, IssueKind};
use crate::pure::route::RouteKind;

/// One query fingerprint's use within a request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FpUse {
    /// Fingerprint hash.
    pub fp: u64,
    /// Executions in the request.
    pub count: u32,
    /// Their total time in microseconds.
    pub time_us: u64,
}

/// One issue within a request, without its timeline indices.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestIssue {
    /// Issue type.
    pub kind: IssueKind,
    /// Stable issue key.
    pub key: u64,
    /// Queries involved.
    pub count: u32,
    /// Their total time in microseconds.
    pub time_us: u64,
    /// Fingerprint hash, `None` for too-many-queries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fp_hash: Option<u64>,
    /// Call site file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Call site line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

impl From<&IssueHit> for RequestIssue {
    fn from(h: &IssueHit) -> Self {
        Self {
            kind: h.kind,
            key: h.key,
            count: h.count,
            time_us: h.time_us,
            fp_hash: h.fp_hash,
            file: h.file.clone(),
            line: h.line,
        }
    }
}

#[allow(clippy::struct_excessive_bools)]
/// A finalised request: the list row, kept in memory for recent requests and
/// written as one `.req` line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinalRequest {
    /// Request id.
    pub id: String,
    /// Yerd site name.
    pub site: String,
    /// The site's PHP version.
    pub php: String,
    /// Request start, Unix epoch milliseconds.
    pub ts_ms: u64,
    /// HTTP method.
    pub method: String,
    /// Request path, query string included.
    pub path: String,
    /// Route id (see [`crate::RouteKey::id`]).
    pub route_id: u64,
    /// Route label.
    pub route_label: String,
    /// How the route was identified.
    pub route_kind: RouteKind,
    /// Framework route name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_name: Option<String>,
    /// Framework action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_action: Option<String>,
    /// Response status.
    pub status: u16,
    /// Proxy-measured duration in microseconds.
    pub duration_us: u64,
    /// Extension-measured duration in microseconds, for display only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ext_duration_us: Option<u64>,
    /// Response body bytes.
    pub bytes: u64,
    /// Response content type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    /// First request of a fresh worker.
    #[serde(default)]
    pub cold: bool,
    /// Client went away early.
    #[serde(default)]
    pub aborted: bool,
    /// Proxy could not reach PHP-FPM.
    #[serde(default)]
    pub proxy_error: bool,
    /// Trace params were sent.
    #[serde(default)]
    pub traced: bool,
    /// An extension summary arrived.
    #[serde(default)]
    pub has_summary: bool,
    /// A timeline with at least one event exists for this request.
    #[serde(default)]
    pub has_detail: bool,
    /// Queries run.
    #[serde(default)]
    pub queries: u32,
    /// Dumps captured.
    #[serde(default)]
    pub dumps: u32,
    /// Log writes captured.
    #[serde(default)]
    pub logs: u32,
    /// Extension frames lost for this request.
    #[serde(default)]
    pub dropped_frames: u32,
    /// Database time, when a summary arrived.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db_us: Option<u64>,
    /// Outgoing HTTP time, when a summary arrived.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_us: Option<u64>,
    /// Time before PHP started, when a summary arrived.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_us: Option<u64>,
    /// Application time, when a summary arrived.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_us: Option<u64>,
    /// Query fingerprints used.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fingerprints: Vec<FpUse>,
    /// Issues detected.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<RequestIssue>,
}

use crate::pure::num::us_to_ms as ms;

impl FinalRequest {
    /// Whether the request counts as failed (5xx or proxy error).
    #[must_use]
    pub fn failed(&self) -> bool {
        self.status >= 500 || self.proxy_error
    }

    /// Whether the request enters latency statistics (not cold, aborted or a
    /// proxy error).
    #[must_use]
    pub fn warm(&self) -> bool {
        !self.cold && !self.aborted && !self.proxy_error
    }

    /// Issue kinds present, deduplicated, in display order.
    #[must_use]
    pub fn issue_kinds(&self) -> Vec<IssueKind> {
        let bits = self.issues.iter().fold(0u8, |acc, i| acc | i.kind.bit());
        IssueKind::from_bits(bits)
    }

    /// Case-insensitive substring match over method, path and route.
    #[must_use]
    pub fn matches(&self, query: &str) -> bool {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return true;
        }
        [
            self.method.as_str(),
            self.path.as_str(),
            self.route_label.as_str(),
            self.route_name.as_deref().unwrap_or(""),
            self.route_action.as_deref().unwrap_or(""),
        ]
        .iter()
        .any(|f| f.to_lowercase().contains(&q))
            || self.status.to_string() == q
    }

    /// The list-row wire shape.
    #[must_use]
    pub fn to_summary(&self) -> yerd_ipc::InsightsRequestSummary {
        yerd_ipc::InsightsRequestSummary {
            id: self.id.clone(),
            ts_ms: self.ts_ms,
            method: self.method.clone(),
            path: self.path.clone(),
            route_label: self.route_label.clone(),
            route_name: self.route_name.clone(),
            route_action: self.route_action.clone(),
            status: self.status,
            duration_ms: ms(self.duration_us),
            ext_duration_ms: self.ext_duration_us.map(ms),
            bytes: self.bytes,
            content_type: self.content_type.clone(),
            cold: self.cold,
            aborted: self.aborted,
            traced: self.traced,
            queries: self.queries,
            dumps: self.dumps,
            logs: self.logs,
            issue_kinds: self
                .issue_kinds()
                .into_iter()
                .map(IssueKind::to_ipc)
                .collect(),
            db_ms: self.db_us.map(ms),
            http_ms: self.http_us.map(ms),
            wait_ms: self.wait_us.map(ms),
            app_ms: self.app_us.map(ms),
        }
    }
}

/// One timeline event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DetailEvent {
    /// Position in the timeline.
    pub idx: u32,
    /// Event category.
    pub category: DumpCategory,
    /// Microseconds from PHP request start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub t_us: Option<u64>,
    /// Duration in microseconds, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dur_us: Option<u64>,
    /// The payload, with long strings capped.
    pub payload: serde_json::Value,
}

/// One request's drill-in timeline, stored as one gzip member in `.det`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestDetail {
    /// Request id, checked on lookup.
    pub id: String,
    /// Timeline events in time order.
    pub events: Vec<DetailEvent>,
    /// Issues with their timeline indices.
    #[serde(default)]
    pub issues: Vec<IssueHit>,
    /// The detail hit its size cap.
    #[serde(default)]
    pub truncated: bool,
}

impl RequestDetail {
    /// An empty timeline for `id`.
    #[must_use]
    pub fn empty(id: &str) -> Self {
        Self {
            id: id.to_owned(),
            events: Vec::new(),
            issues: Vec::new(),
            truncated: false,
        }
    }

    /// The drill-in wire shape, with `summary` as its header.
    #[must_use]
    pub fn to_ipc(&self, summary: &FinalRequest) -> yerd_ipc::InsightsRequestDetail {
        yerd_ipc::InsightsRequestDetail {
            summary: summary.to_summary(),
            events: self
                .events
                .iter()
                .map(|e| yerd_ipc::InsightsEvent {
                    idx: e.idx,
                    category: e.category,
                    t_us: e.t_us,
                    dur_us: e.dur_us,
                    payload: e.payload.clone(),
                })
                .collect(),
            issues: self
                .issues
                .iter()
                .map(|h| yerd_ipc::InsightsRequestIssue {
                    kind: h.kind.to_ipc(),
                    key: format!("{:016x}", h.key),
                    event_indices: h.event_indices.clone(),
                    count: h.count,
                    time_ms: ms(h.time_us),
                    file: h.file.clone(),
                    line: h.line,
                    hint: hint(h.kind).to_owned(),
                })
                .collect(),
            truncated: self.truncated,
        }
    }
}

/// Format an issue key for the wire: 16 lowercase hex chars.
#[must_use]
pub fn key_hex(key: u64) -> String {
    format!("{key:016x}")
}

/// Parse a wire issue key.
#[must_use]
pub fn parse_key_hex(s: &str) -> Option<u64> {
    if s.len() != 16 {
        return None;
    }
    u64::from_str_radix(s, 16).ok()
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn request(id: &str, ts_ms: u64, duration_us: u64) -> FinalRequest {
        FinalRequest {
            id: id.to_owned(),
            site: "blog".to_owned(),
            php: "8.3".to_owned(),
            ts_ms,
            method: "GET".to_owned(),
            path: "/orders/1".to_owned(),
            route_id: 7,
            route_label: "/orders/{id}".to_owned(),
            route_kind: RouteKind::Path,
            route_name: None,
            route_action: None,
            status: 200,
            duration_us,
            ext_duration_us: None,
            bytes: 10,
            content_type: None,
            cold: false,
            aborted: false,
            proxy_error: false,
            traced: false,
            has_summary: false,
            has_detail: false,
            queries: 0,
            dumps: 0,
            logs: 0,
            dropped_frames: 0,
            db_us: None,
            http_us: None,
            wait_us: None,
            app_us: None,
            fingerprints: Vec::new(),
            issues: Vec::new(),
        }
    }

    #[test]
    fn summary_conversion() {
        let mut r = request("a", 5, 1_500);
        r.db_us = Some(500);
        r.issues = vec![
            RequestIssue {
                kind: IssueKind::SlowQuery,
                key: 1,
                count: 1,
                time_us: 1,
                fp_hash: None,
                file: None,
                line: None,
            },
            RequestIssue {
                kind: IssueKind::NPlusOne,
                key: 2,
                count: 5,
                time_us: 1,
                fp_hash: None,
                file: None,
                line: None,
            },
            RequestIssue {
                kind: IssueKind::SlowQuery,
                key: 3,
                count: 1,
                time_us: 1,
                fp_hash: None,
                file: None,
                line: None,
            },
        ];
        let s = r.to_summary();
        assert!((s.duration_ms - 1.5).abs() < f64::EPSILON);
        assert_eq!(s.db_ms, Some(0.5));
        assert_eq!(
            s.issue_kinds,
            vec![
                yerd_ipc::InsightsIssueKind::NPlusOne,
                yerd_ipc::InsightsIssueKind::SlowQuery
            ]
        );
    }

    #[test]
    fn query_matching() {
        let r = request("a", 5, 1);
        let cases: &[(&str, bool)] = &[
            ("", true),
            ("orders", true),
            ("ORDERS/{ID}", true),
            ("get", true),
            ("200", true),
            ("post", false),
        ];
        for (q, want) in cases {
            assert_eq!(r.matches(q), *want, "{q}");
        }
    }

    #[test]
    fn key_hex_round_trip() {
        assert_eq!(key_hex(255), "00000000000000ff");
        assert_eq!(parse_key_hex("00000000000000ff"), Some(255));
        assert_eq!(parse_key_hex("ff"), None);
        assert_eq!(parse_key_hex("zzzzzzzzzzzzzzzz"), None);
    }
}
