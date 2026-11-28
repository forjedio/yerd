//! The proxy and extension inputs (contract C1 to C6).
//!
//! The proxy hands the extension a request id through `FastCGI` params
//! ([`PARAM_TRACE_ID`] and friends); the extension tags every frame with it
//! plus `v`, `seq` and `t_us`, and ends with a `request` summary
//! ([`ExtSummary`]). The daemon maps both sides into these types.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use yerd_ipc::DumpCategory;

/// INI directive selecting the extension's protocol (C1).
pub const PROTOCOL_INI: &str = "yerd_dump.protocol";

/// The trace-mode protocol version Yerd selects and routes to Insights.
pub const PROTOCOL_VERSION: u8 = 2;

/// `FastCGI` param carrying the request id (C2).
pub const PARAM_TRACE_ID: &str = "YERD_TRACE_ID";

/// `FastCGI` param carrying the dump-server loopback port (C2).
pub const PARAM_TRACE_PORT: &str = "YERD_TRACE_PORT";

/// `FastCGI` param carrying the Yerd site name (C2).
pub const PARAM_TRACE_SITE: &str = "YERD_TRACE_SITE";

/// `FastCGI` param carrying the comma-separated capture features (C2).
pub const PARAM_TRACE_FEATURES: &str = "YERD_TRACE_FEATURES";

/// Every capture feature key, in the order [`features_param`] emits them.
pub const FEATURE_KEYS: [&str; 8] = [
    "dumps", "queries", "jobs", "views", "requests", "logs", "cache", "http",
];

/// One proxy-timed PHP request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxySample {
    /// The request id (see [`trace_id`]).
    pub id: String,
    /// Yerd site name.
    pub site: String,
    /// The site's PHP version (`"8.3"`).
    pub php: String,
    /// HTTP method.
    pub method: String,
    /// Request path, query string included.
    pub path: String,
    /// Executed script relative to the document root, `None` for
    /// front-controller requests.
    pub script: Option<String>,
    /// Response status.
    pub status: u16,
    /// Response body bytes sent.
    pub bytes: u64,
    /// Response content type, at most 64 bytes.
    pub content_type: Option<String>,
    /// Wall-clock start, Unix epoch microseconds.
    pub start_us: u64,
    /// Proxy-measured duration in microseconds, the authoritative one.
    pub duration_us: u64,
    /// The client went away before the body finished.
    pub aborted: bool,
    /// The proxy could not talk to PHP-FPM; PHP never ran.
    pub proxy_error: bool,
    /// Whether trace params were sent, so the extension may narrate it.
    pub traced: bool,
}

/// One protocol-2 extension frame.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtFrame {
    /// The request id from `YERD_TRACE_ID`.
    pub request_id: String,
    /// The Yerd site name from `YERD_TRACE_SITE`.
    pub site: String,
    /// Frame category.
    pub category: DumpCategory,
    /// Per-request sequence number.
    pub seq: Option<u32>,
    /// Microseconds from PHP request start.
    pub t_us: Option<u64>,
    /// Category-specific payload.
    pub payload: Value,
}

/// The matched framework route from a request summary.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtRoute {
    /// Route template as the framework stores it (`orders/{order}`).
    pub uri: Option<String>,
    /// Route name.
    pub name: Option<String>,
    /// `Controller@method` or `Closure`.
    pub action: Option<String>,
}

/// The extension's request summary (C4). Every field defaults when missing or
/// mistyped.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExtSummary {
    /// HTTP method.
    pub method: String,
    /// Request URI.
    pub uri: String,
    /// Response status.
    pub status: u16,
    /// Extension-measured duration in ms, for display.
    pub duration_ms: f64,
    /// Wall-clock start at RINIT, Unix epoch microseconds.
    pub start_us: u64,
    /// Microseconds from RINIT to the summary.
    pub duration_us: u64,
    /// FPM worker pid.
    pub worker_pid: u64,
    /// Requests this worker has handled, this one included; `1` is a cold start.
    pub worker_requests: u64,
    /// Matched route.
    pub route: Option<ExtRoute>,
    /// Frames built for this request, the summary included.
    pub frames: u32,
    /// Frames built but not written.
    pub frames_unsent: u32,
}

fn str_field(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_owned()
}

fn u64_field(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn opt_str(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_owned)
}

impl ExtSummary {
    /// Parse a `request` frame payload. `None` only when it is not an object.
    #[must_use]
    pub fn from_payload(payload: &Value) -> Option<Self> {
        payload.as_object()?;
        let route = payload
            .get("route")
            .filter(|r| r.is_object())
            .map(|r| ExtRoute {
                uri: opt_str(r, "uri"),
                name: opt_str(r, "name"),
                action: opt_str(r, "action"),
            });
        Some(Self {
            method: str_field(payload, "method"),
            uri: str_field(payload, "uri"),
            status: u16::try_from(u64_field(payload, "status")).unwrap_or(0),
            duration_ms: payload
                .get("duration_ms")
                .and_then(Value::as_f64)
                .unwrap_or(0.0),
            start_us: u64_field(payload, "start_us"),
            duration_us: u64_field(payload, "duration_us"),
            worker_pid: u64_field(payload, "worker_pid"),
            worker_requests: u64_field(payload, "worker_requests"),
            route,
            frames: u32::try_from(u64_field(payload, "frames")).unwrap_or(u32::MAX),
            frames_unsent: u32::try_from(u64_field(payload, "frames_unsent")).unwrap_or(u32::MAX),
        })
    }
}

/// A request id: a 16-hex boot nonce followed by a 16-hex counter.
#[must_use]
pub fn trace_id(nonce: u64, n: u64) -> String {
    format!("{nonce:016x}{n:016x}")
}

/// The `YERD_TRACE_FEATURES` value for resolved feature flags: the enabled
/// keys in [`FEATURE_KEYS`] order. An absent key counts as on, matching the
/// config's semantics; unknown keys are ignored.
#[must_use]
pub fn features_param(features: &BTreeMap<String, bool>) -> String {
    FEATURE_KEYS
        .iter()
        .filter(|k| features.get(**k).copied().unwrap_or(true))
        .copied()
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn param_names_match_the_contract() {
        assert_eq!(PROTOCOL_INI, "yerd_dump.protocol");
        assert_eq!(PROTOCOL_VERSION, 2);
        assert_eq!(
            [
                PARAM_TRACE_ID,
                PARAM_TRACE_PORT,
                PARAM_TRACE_SITE,
                PARAM_TRACE_FEATURES
            ],
            [
                "YERD_TRACE_ID",
                "YERD_TRACE_PORT",
                "YERD_TRACE_SITE",
                "YERD_TRACE_FEATURES"
            ]
        );
    }

    #[test]
    fn trace_id_format() {
        let cases: &[(u64, u64, &str)] = &[
            (0, 0, "00000000000000000000000000000000"),
            (0xdead_beef, 1, "00000000deadbeef0000000000000001"),
            (u64::MAX, u64::MAX, "ffffffffffffffffffffffffffffffff"),
        ];
        for (nonce, n, want) in cases {
            let id = trace_id(*nonce, *n);
            assert_eq!(id, *want);
            assert_eq!(id.len(), 32);
        }
    }

    #[test]
    fn full_summary_payload() {
        let p = json!({
            "method": "GET", "uri": "/orders/1", "status": 200, "duration_ms": 12.5, "ip": "127.0.0.1",
            "start_us": 1_700_000_000_000_000u64, "duration_us": 12_500, "worker_pid": 4242,
            "worker_requests": 1,
            "route": {"uri": "orders/{order}", "name": "orders.show", "action": "OrderController@show"},
            "frames": 7, "frames_unsent": 1
        });
        let s = ExtSummary::from_payload(&p).unwrap();
        assert_eq!(s.method, "GET");
        assert_eq!(s.uri, "/orders/1");
        assert_eq!(s.status, 200);
        assert!((s.duration_ms - 12.5).abs() < f64::EPSILON);
        assert_eq!(s.start_us, 1_700_000_000_000_000);
        assert_eq!(s.duration_us, 12_500);
        assert_eq!(s.worker_pid, 4242);
        assert_eq!(s.worker_requests, 1);
        assert_eq!(
            s.route,
            Some(ExtRoute {
                uri: Some("orders/{order}".into()),
                name: Some("orders.show".into()),
                action: Some("OrderController@show".into()),
            })
        );
        assert_eq!(s.frames, 7);
        assert_eq!(s.frames_unsent, 1);
    }

    #[test]
    fn missing_null_and_mistyped_fields_default() {
        let cases = [
            (json!({}), None, 0),
            (json!({"route": null, "frames": 3}), None, 3),
            (
                json!({"route": "x", "frames": "3", "status": "200"}),
                None,
                0,
            ),
            (
                json!({"route": {"uri": 5, "name": "n"}, "frames": -1}),
                Some(ExtRoute {
                    uri: None,
                    name: Some("n".into()),
                    action: None,
                }),
                0,
            ),
        ];
        for (p, route, frames) in cases {
            let s = ExtSummary::from_payload(&p).unwrap();
            assert_eq!(s.route, route, "{p}");
            assert_eq!(s.frames, frames, "{p}");
            assert_eq!(s.status, 0, "{p}");
            assert_eq!(s.worker_requests, 0, "{p}");
        }
        assert!(ExtSummary::from_payload(&json!([1])).is_none());
        assert!(ExtSummary::from_payload(&json!(null)).is_none());
    }

    #[test]
    fn feature_param_ordering() {
        let mk = |pairs: &[(&str, bool)]| {
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), *v))
                .collect::<BTreeMap<_, _>>()
        };
        let cases: Vec<(BTreeMap<String, bool>, &str)> = vec![
            (
                BTreeMap::new(),
                "dumps,queries,jobs,views,requests,logs,cache,http",
            ),
            (
                mk(&[("http", true), ("dumps", false), ("queries", true)]),
                "queries,jobs,views,requests,logs,cache,http",
            ),
            (mk(&FEATURE_KEYS.map(|k| (k, false))), ""),
            (
                mk(&[("bogus", true), ("cache", false)]),
                "dumps,queries,jobs,views,requests,logs,http",
            ),
        ];
        for (features, want) in cases {
            assert_eq!(features_param(&features), want);
        }
    }
}
