//! Insights wire types: per-site request performance and query problems.
//!
//! The daemon aggregates proxy timings and extension frames per site and
//! serves these shapes to the GUI. Issue keys travel as 16-char lowercase hex
//! strings rather than JSON numbers, because a `u64` key does not survive a
//! JavaScript `number`.
//!
//! Wire shapes are pinned in `tests/wire_stability.rs`.

use serde::{Deserialize, Serialize};

use crate::dump::DumpCategory;

/// The time range an Insights query covers.
///
/// Serialises as `"5m"`, `"1h"` or `"24h"`. Those strings are not valid Rust
/// identifiers, so the mapping goes through `String` instead of a serde rename.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
#[non_exhaustive]
pub enum InsightsRange {
    /// The last five minutes, served exactly from in-memory requests.
    FiveMinutes,
    /// The last hour, from minute rollups.
    OneHour,
    /// The last 24 hours, from minute rollups.
    OneDay,
}

impl InsightsRange {
    /// The wire string for this range.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FiveMinutes => "5m",
            Self::OneHour => "1h",
            Self::OneDay => "24h",
        }
    }

    /// The range length in milliseconds.
    #[must_use]
    pub fn millis(self) -> u64 {
        match self {
            Self::FiveMinutes => 300_000,
            Self::OneHour => 3_600_000,
            Self::OneDay => 86_400_000,
        }
    }
}

impl From<InsightsRange> for String {
    fn from(r: InsightsRange) -> Self {
        r.as_str().to_owned()
    }
}

impl TryFrom<String> for InsightsRange {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        match s.as_str() {
            "5m" => Ok(Self::FiveMinutes),
            "1h" => Ok(Self::OneHour),
            "24h" => Ok(Self::OneDay),
            other => Err(format!("unknown insights range `{other}`")),
        }
    }
}

/// Request-list ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum InsightsSort {
    /// Newest first.
    Recent,
    /// Longest duration first.
    Slowest,
}

/// How a route was identified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum InsightsRouteKind {
    /// A framework route template (Laravel `orders/{order}`).
    Template,
    /// A directly executed PHP script (non-front-controller sites).
    Script,
    /// A normalised URL path.
    Path,
}

/// A detected issue type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum InsightsIssueKind {
    /// The same query shape repeated from one call site with differing values.
    NPlusOne,
    /// The identical query (same SQL and bindings) ran more than once.
    Duplicate,
    /// A query slower than the threshold.
    SlowQuery,
    /// A read without `LIMIT` returning many rows.
    LargeRead,
    /// A request running more queries than the threshold.
    TooManyQueries,
}

/// One part of the "where the time goes" breakdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum InsightsTimePart {
    /// PHP application time (total minus the other parts).
    App,
    /// Database query time.
    Db,
    /// Outgoing HTTP client time.
    Http,
    /// Time before PHP started (pool spawn, `FastCGI` queueing).
    Wait,
}

/// Issue-detection thresholds, as carried on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct InsightsThresholds {
    /// Minimum repeats of one query shape from one call site to flag an N+1.
    pub n_plus_one_min: u32,
    /// Minimum identical queries to flag a duplicate (at least 2).
    pub duplicate_min: u32,
    /// A query slower than this many milliseconds is slow.
    pub slow_query_ms: u32,
    /// A read without `LIMIT` returning at least this many rows is large.
    pub large_read_rows: u32,
    /// A request running more than this many queries is flagged.
    pub max_queries: u32,
}

impl Default for InsightsThresholds {
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

/// The Overview tab for one site and range.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InsightsOverview {
    /// The range this overview covers.
    pub range: InsightsRange,
    /// The KPI tiles.
    pub kpis: InsightsKpis,
    /// Cold-start requests, counted but excluded from latency statistics.
    pub cold: u64,
    /// Proxy samples lost (channel full, failed disk writes).
    pub dropped_requests: u64,
    /// Extension frames lost (missing frames, orphans, caps).
    pub dropped_frames: u64,
    /// Whether any request in the range carried an extension summary.
    pub ext_seen: bool,
    /// Whether the site's PHP version can load the extension (not legacy).
    pub ext_available: bool,
    /// Latency over time.
    pub series: Vec<InsightsPoint>,
    /// Where the time of an average warm request goes.
    pub breakdown: Vec<InsightsSlice>,
    /// Routes by total time.
    pub routes: Vec<InsightsRoute>,
    /// Query fingerprints by total time.
    pub queries: Vec<InsightsQuery>,
    /// Detected issues by impact, dismissed ones included and flagged.
    pub issues: Vec<InsightsIssue>,
}

impl InsightsOverview {
    /// An overview with no data, for a site that has recorded nothing.
    #[must_use]
    pub fn empty(range: InsightsRange) -> Self {
        Self {
            range,
            kpis: InsightsKpis::default(),
            cold: 0,
            dropped_requests: 0,
            dropped_frames: 0,
            ext_seen: false,
            ext_available: true,
            series: Vec::new(),
            breakdown: Vec::new(),
            routes: Vec::new(),
            queries: Vec::new(),
            issues: Vec::new(),
        }
    }
}

/// The Overview's KPI tiles.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct InsightsKpis {
    /// Warm median latency.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p50_ms: Option<f64>,
    /// Warm 95th-percentile latency.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p95_ms: Option<f64>,
    /// Median change against the previous equal window (5m and 1h only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p50_delta_ms: Option<f64>,
    /// The highest per-point p95 in the series.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p95_spike_ms: Option<f64>,
    /// When that spike happened, Unix epoch milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p95_spike_at_ms: Option<u64>,
    /// Requests in the range.
    pub requests: u64,
    /// Mean requests per minute over the range.
    pub per_minute: f64,
    /// Static files served without PHP, not counted as requests.
    pub static_skipped: u64,
    /// Failed requests (5xx or proxy error) as a percentage.
    pub failed_pct: f64,
    /// Requests answered 5xx (proxy errors included).
    pub s5xx: u64,
    /// Requests answered 4xx.
    pub s4xx: u64,
    /// Mean queries per request with an extension summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queries_avg: Option<f64>,
    /// The most queries any one request ran.
    pub queries_max: u32,
    /// The route of that request, as `METHOD label`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queries_max_route: Option<String>,
}

/// One point of the latency chart.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InsightsPoint {
    /// Bucket start, Unix epoch milliseconds.
    pub t_ms: u64,
    /// Requests in the bucket.
    pub count: u64,
    /// Warm median latency in the bucket.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p50_ms: Option<f64>,
    /// Warm 95th-percentile latency in the bucket.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p95_ms: Option<f64>,
    /// Failed requests in the bucket.
    pub errors: u64,
}

/// One part of the time breakdown.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InsightsSlice {
    /// Which part.
    pub part: InsightsTimePart,
    /// Fraction of the total, `0.0..=1.0`.
    pub share: f64,
    /// Mean milliseconds per request.
    pub mean_ms: f64,
}

/// One row of the routes table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InsightsRoute {
    /// HTTP method.
    pub method: String,
    /// Display label (`/orders/{order}`).
    pub label: String,
    /// How the route was identified.
    pub kind: InsightsRouteKind,
    /// Framework route name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Framework action (`Controller@method`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    /// Requests in the range.
    pub requests: u64,
    /// Warm median latency.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p50_ms: Option<f64>,
    /// Warm 95th-percentile latency.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p95_ms: Option<f64>,
    /// Fraction of the site's total time spent in this route.
    pub share: f64,
    /// Mean queries per request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queries_per_request: Option<f64>,
    /// Issue types seen on this route.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issue_kinds: Vec<InsightsIssueKind>,
}

/// One row of the queries table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InsightsQuery {
    /// The normalised SQL.
    pub fingerprint: String,
    /// Executions in the range.
    pub count: u64,
    /// Total time.
    pub total_ms: f64,
    /// Mean time per execution.
    pub mean_ms: f64,
    /// The route that ran it most recently, as `METHOD label`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_label: Option<String>,
}

/// A detected issue, aggregated over the range.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InsightsIssue {
    /// Stable issue key, 16 lowercase hex chars.
    pub key: String,
    /// Issue type.
    pub kind: InsightsIssueKind,
    /// The query's normalised SQL, when the issue concerns one query shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sql: Option<String>,
    /// A short fix hint.
    pub hint: String,
    /// Application call site file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Application call site line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// The route's HTTP method.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_method: Option<String>,
    /// The route's label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_label: Option<String>,
    /// How many times it happened.
    pub occurrences: u64,
    /// How many requests it affected.
    pub affected_requests: u64,
    /// Total query time involved.
    pub impact_ms: f64,
    /// The most occurrences within one request.
    pub max_in_request: u32,
    /// The most recent affected request's id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_request_id: Option<String>,
    /// The most recent affected request's start, Unix epoch milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_request_ts_ms: Option<u64>,
    /// Whether the user dismissed it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dismissed: bool,
}

/// One row of the request list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InsightsRequestSummary {
    /// The request id (32 hex chars).
    pub id: String,
    /// Request start, Unix epoch milliseconds.
    pub ts_ms: u64,
    /// HTTP method.
    pub method: String,
    /// Request path, query string included.
    pub path: String,
    /// The route label.
    pub route_label: String,
    /// Framework route name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_name: Option<String>,
    /// Framework action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_action: Option<String>,
    /// Response status.
    pub status: u16,
    /// Proxy-measured duration, the authoritative one.
    pub duration_ms: f64,
    /// Extension-measured duration, for display only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ext_duration_ms: Option<f64>,
    /// Response body bytes.
    pub bytes: u64,
    /// Response content type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    /// Served by a freshly spawned worker, so excluded from statistics.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cold: bool,
    /// The client went away before the response finished.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub aborted: bool,
    /// Whether the extension was asked to trace this request.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub traced: bool,
    /// Queries run.
    pub queries: u32,
    /// Dumps captured.
    pub dumps: u32,
    /// Log writes captured.
    pub logs: u32,
    /// Issue types detected in this request.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issue_kinds: Vec<InsightsIssueKind>,
    /// Database time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db_ms: Option<f64>,
    /// Outgoing HTTP time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_ms: Option<f64>,
    /// Time before PHP started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_ms: Option<f64>,
    /// Application time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_ms: Option<f64>,
}

/// One request's drill-in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InsightsRequestDetail {
    /// The list row.
    pub summary: InsightsRequestSummary,
    /// The event timeline, in time order.
    pub events: Vec<InsightsEvent>,
    /// Issues detected in this request.
    pub issues: Vec<InsightsRequestIssue>,
    /// True when the detail hit its size cap and events were cut.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

/// One timeline event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InsightsEvent {
    /// Position in the request's event list.
    pub idx: u32,
    /// Event category.
    pub category: DumpCategory,
    /// Microseconds from PHP request start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub t_us: Option<u64>,
    /// Event duration in microseconds, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dur_us: Option<u64>,
    /// The extension's payload, with long strings capped.
    pub payload: serde_json::Value,
}

/// An issue detected within one request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InsightsRequestIssue {
    /// Issue type.
    pub kind: InsightsIssueKind,
    /// Stable issue key, 16 lowercase hex chars.
    pub key: String,
    /// The [`InsightsEvent::idx`] values involved.
    pub event_indices: Vec<u32>,
    /// How many events are involved.
    pub count: u32,
    /// Their total time.
    pub time_ms: f64,
    /// Application call site file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Application call site line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// A short fix hint.
    pub hint: String,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn range_round_trips_and_rejects_unknown() {
        for (r, s) in [
            (InsightsRange::FiveMinutes, "\"5m\""),
            (InsightsRange::OneHour, "\"1h\""),
            (InsightsRange::OneDay, "\"24h\""),
        ] {
            assert_eq!(serde_json::to_string(&r).unwrap(), s);
            assert_eq!(serde_json::from_str::<InsightsRange>(s).unwrap(), r);
        }
        assert!(serde_json::from_str::<InsightsRange>("\"7d\"").is_err());
    }

    #[test]
    fn range_millis() {
        assert_eq!(InsightsRange::FiveMinutes.millis(), 300_000);
        assert_eq!(InsightsRange::OneHour.millis(), 3_600_000);
        assert_eq!(InsightsRange::OneDay.millis(), 86_400_000);
    }
}
