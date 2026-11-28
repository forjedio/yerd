//! Per-minute rollups and the Overview computed from them.
//!
//! A [`MinuteRollup`] holds only fixed-size numbers and `u64` keys; labels and
//! SQL text live in the site's [`Dict`]. Rollups are mergeable by addition.
//! The per-minute maps are capped (50 routes, 30 fingerprints, 30 issues):
//! route overflow folds into [`OTHER_ROUTE`], fingerprint and issue overflow
//! is counted in `capped` and dropped. Merging capped maps is therefore an
//! approximation; it is exact while every map stays under its cap.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use yerd_ipc::{
    InsightsIssue, InsightsKpis, InsightsOverview, InsightsPoint, InsightsQuery, InsightsRange,
    InsightsRoute, InsightsRouteKind, InsightsSlice, InsightsTimePart,
};

use crate::pure::dict::{Dict, DictEntry};
use crate::pure::hist::LatencyHist;
use crate::pure::issues::{hint, IssueKind};
use crate::pure::model::{key_hex, FinalRequest};
use crate::pure::num::{f, us_to_ms};
use crate::pure::route::OTHER_ROUTE;

/// Most routes kept per minute before folding into [`OTHER_ROUTE`].
pub const ROUTES_CAP: usize = 50;
/// Most fingerprints kept per minute.
pub const FINGERPRINTS_CAP: usize = 30;
/// Most issues kept per minute.
pub const ISSUES_CAP: usize = 30;
/// One minute in milliseconds.
pub const MINUTE_MS: u64 = 60_000;
/// Rows returned in the Overview's routes and queries tables.
pub const TOP_ROWS: usize = 50;
/// Issues returned in the Overview.
pub const TOP_ISSUES: usize = 100;

/// Per-route aggregate within a minute.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteAgg {
    /// Requests.
    pub requests: u64,
    /// Failed requests.
    #[serde(default)]
    pub s5xx: u64,
    /// Warm latencies.
    #[serde(default)]
    pub warm_hist: LatencyHist,
    /// Total duration in microseconds.
    #[serde(default)]
    pub sum_us: u64,
    /// Queries run by requests with an extension summary.
    #[serde(default)]
    pub queries: u64,
    /// Requests with an extension summary.
    #[serde(default)]
    pub ext_requests: u64,
    /// Issue kinds seen, as [`IssueKind::bit`] flags.
    #[serde(default)]
    pub issue_kinds: u8,
}

impl RouteAgg {
    fn merge(&mut self, o: &Self) {
        self.requests += o.requests;
        self.s5xx += o.s5xx;
        self.warm_hist.merge(&o.warm_hist);
        self.sum_us = self.sum_us.saturating_add(o.sum_us);
        self.queries += o.queries;
        self.ext_requests += o.ext_requests;
        self.issue_kinds |= o.issue_kinds;
    }
}

/// Per-fingerprint aggregate within a minute.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FpAgg {
    /// Executions.
    pub count: u64,
    /// Total time in microseconds.
    pub sum_us: u64,
    /// The route that last ran it.
    pub route_id: u64,
}

/// Per-issue aggregate within a minute.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueAgg {
    /// Occurrences (queries involved).
    pub occurrences: u64,
    /// Requests affected.
    pub affected_requests: u64,
    /// Total time involved in microseconds.
    pub sum_us: u64,
    /// Most occurrences within one request.
    pub max_in_request: u32,
    /// Most recent affected request: `(id, ts_ms)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_request: Option<(String, u64)>,
}

impl IssueAgg {
    fn merge(&mut self, o: &Self) {
        self.occurrences += o.occurrences;
        self.affected_requests += o.affected_requests;
        self.sum_us = self.sum_us.saturating_add(o.sum_us);
        self.max_in_request = self.max_in_request.max(o.max_in_request);
        if let Some(theirs) = &o.last_request {
            if self
                .last_request
                .as_ref()
                .map_or(true, |mine| theirs.1 >= mine.1)
            {
                self.last_request = Some(theirs.clone());
            }
        }
    }
}

/// One minute of one site's activity.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MinuteRollup {
    /// Minute start, Unix epoch milliseconds.
    #[serde(rename = "m")]
    pub minute: u64,
    /// Requests.
    #[serde(default)]
    pub requests: u64,
    /// Cold-start requests.
    #[serde(default)]
    pub cold: u64,
    /// Aborted requests.
    #[serde(default)]
    pub aborted: u64,
    /// Failed requests (5xx or proxy error).
    #[serde(default)]
    pub s5xx: u64,
    /// 4xx requests.
    #[serde(default)]
    pub s4xx: u64,
    /// Warm latencies.
    #[serde(default)]
    pub warm_hist: LatencyHist,
    /// Requests with an extension summary.
    #[serde(default)]
    pub ext_requests: u64,
    /// Queries run by those requests.
    #[serde(default)]
    pub queries_total: u64,
    /// The most queries one request ran, and its route.
    #[serde(default)]
    pub max_queries: (u32, u64),
    /// Warm requests with an extension summary, the breakdown denominator.
    #[serde(default)]
    pub breakdown_requests: u64,
    /// Summed app time of those requests.
    #[serde(default)]
    pub sum_app_us: u64,
    /// Summed database time of those requests.
    #[serde(default)]
    pub sum_db_us: u64,
    /// Summed outgoing HTTP time of those requests.
    #[serde(default)]
    pub sum_http_us: u64,
    /// Summed wait time of those requests.
    #[serde(default)]
    pub sum_wait_us: u64,
    /// Static files served without PHP.
    #[serde(default)]
    pub static_skipped: u64,
    /// Proxy samples lost.
    #[serde(default)]
    pub dropped_requests: u64,
    /// Extension frames lost.
    #[serde(default)]
    pub dropped_frames: u64,
    /// Fingerprint and issue entries dropped by the per-minute caps.
    #[serde(default)]
    pub capped: u64,
    /// Per-route aggregates.
    #[serde(default)]
    pub routes: BTreeMap<u64, RouteAgg>,
    /// Per-fingerprint aggregates.
    #[serde(default)]
    pub fingerprints: BTreeMap<u64, FpAgg>,
    /// Per-issue aggregates.
    #[serde(default)]
    pub issues: BTreeMap<u64, IssueAgg>,
}

/// The minute start containing `ms`.
#[must_use]
pub fn minute_of(ms: u64) -> u64 {
    ms - ms % MINUTE_MS
}

impl MinuteRollup {
    /// An empty rollup for the minute containing `ms`.
    #[must_use]
    pub fn new(ms: u64) -> Self {
        Self {
            minute: minute_of(ms),
            ..Self::default()
        }
    }

    /// Whether nothing was recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::new(self.minute)
    }

    /// Fold one finalised request in.
    pub fn fold_request(&mut self, r: &FinalRequest) {
        self.requests += 1;
        if r.cold {
            self.cold += 1;
        }
        if r.aborted {
            self.aborted += 1;
        }
        if r.failed() {
            self.s5xx += 1;
        } else if (400..500).contains(&r.status) {
            self.s4xx += 1;
        }
        let warm = r.warm();
        if warm {
            self.warm_hist.record(r.duration_us);
        }
        if r.has_summary {
            self.ext_requests += 1;
            self.queries_total += u64::from(r.queries);
            if r.queries > self.max_queries.0 {
                self.max_queries = (r.queries, r.route_id);
            }
        }
        if let (true, Some(app), Some(db), Some(http), Some(wait)) =
            (warm, r.app_us, r.db_us, r.http_us, r.wait_us)
        {
            self.breakdown_requests += 1;
            self.sum_app_us = self.sum_app_us.saturating_add(app);
            self.sum_db_us = self.sum_db_us.saturating_add(db);
            self.sum_http_us = self.sum_http_us.saturating_add(http);
            self.sum_wait_us = self.sum_wait_us.saturating_add(wait);
        }
        self.dropped_frames += u64::from(r.dropped_frames);
        let bits = r.issues.iter().fold(0u8, |acc, i| acc | i.kind.bit());
        let mut agg = RouteAgg {
            requests: 1,
            s5xx: u64::from(r.failed()),
            warm_hist: LatencyHist::new(),
            sum_us: r.duration_us,
            queries: if r.has_summary {
                u64::from(r.queries)
            } else {
                0
            },
            ext_requests: u64::from(r.has_summary),
            issue_kinds: bits,
        };
        if warm {
            agg.warm_hist.record(r.duration_us);
        }
        self.merge_route(r.route_id, &agg);
        for fp in &r.fingerprints {
            self.merge_fp(
                fp.fp,
                &FpAgg {
                    count: u64::from(fp.count),
                    sum_us: fp.time_us,
                    route_id: r.route_id,
                },
            );
        }
        for i in &r.issues {
            self.merge_issue(
                i.key,
                &IssueAgg {
                    occurrences: u64::from(i.count),
                    affected_requests: 1,
                    sum_us: i.time_us,
                    max_in_request: i.count,
                    last_request: Some((r.id.clone(), r.ts_ms)),
                },
            );
        }
    }

    fn merge_route(&mut self, id: u64, agg: &RouteAgg) {
        let named = self.routes.len() - usize::from(self.routes.contains_key(&OTHER_ROUTE));
        let id = if self.routes.contains_key(&id) || named < ROUTES_CAP {
            id
        } else {
            OTHER_ROUTE
        };
        self.routes.entry(id).or_default().merge(agg);
    }

    fn merge_fp(&mut self, fp: u64, agg: &FpAgg) {
        if let Some(mine) = self.fingerprints.get_mut(&fp) {
            mine.count += agg.count;
            mine.sum_us = mine.sum_us.saturating_add(agg.sum_us);
            mine.route_id = agg.route_id;
        } else if self.fingerprints.len() < FINGERPRINTS_CAP {
            self.fingerprints.insert(fp, agg.clone());
        } else {
            self.capped += 1;
        }
    }

    fn merge_issue(&mut self, key: u64, agg: &IssueAgg) {
        if let Some(mine) = self.issues.get_mut(&key) {
            mine.merge(agg);
        } else if self.issues.len() < ISSUES_CAP {
            self.issues.insert(key, agg.clone());
        } else {
            self.capped += 1;
        }
    }

    /// Add `other` into `self` (see the module docs for the capped case).
    pub fn merge(&mut self, o: &Self) {
        self.requests += o.requests;
        self.cold += o.cold;
        self.aborted += o.aborted;
        self.s5xx += o.s5xx;
        self.s4xx += o.s4xx;
        self.warm_hist.merge(&o.warm_hist);
        self.ext_requests += o.ext_requests;
        self.queries_total += o.queries_total;
        if o.max_queries.0 > self.max_queries.0 {
            self.max_queries = o.max_queries;
        }
        self.breakdown_requests += o.breakdown_requests;
        self.sum_app_us = self.sum_app_us.saturating_add(o.sum_app_us);
        self.sum_db_us = self.sum_db_us.saturating_add(o.sum_db_us);
        self.sum_http_us = self.sum_http_us.saturating_add(o.sum_http_us);
        self.sum_wait_us = self.sum_wait_us.saturating_add(o.sum_wait_us);
        self.static_skipped += o.static_skipped;
        self.dropped_requests += o.dropped_requests;
        self.dropped_frames += o.dropped_frames;
        self.capped += o.capped;
        for (id, agg) in &o.routes {
            self.merge_route(*id, agg);
        }
        for (fp, agg) in &o.fingerprints {
            self.merge_fp(*fp, agg);
        }
        for (key, agg) in &o.issues {
            self.merge_issue(*key, agg);
        }
    }

    /// Every dictionary key this rollup references directly.
    pub fn collect_keys(&self, keys: &mut BTreeSet<u64>) {
        keys.extend(self.routes.keys().copied());
        if self.max_queries.0 > 0 {
            keys.insert(self.max_queries.1);
        }
        for (fp, agg) in &self.fingerprints {
            keys.insert(*fp);
            keys.insert(agg.route_id);
        }
        keys.extend(self.issues.keys().copied());
    }
}

/// The transitive closure of dictionary keys referenced by `rollups`: route
/// ids (including the max-queries route), fingerprint hashes and their routes,
/// issue keys, and each referenced issue's fingerprint and route.
#[must_use]
pub fn referenced_keys<'a>(
    rollups: impl IntoIterator<Item = &'a MinuteRollup>,
    dict: &Dict,
) -> BTreeSet<u64> {
    let mut keys = BTreeSet::new();
    for r in rollups {
        r.collect_keys(&mut keys);
    }
    dict.extend_issue_refs(&mut keys);
    keys
}

/// Every issue key present in `rollups`.
#[must_use]
pub fn open_issue_keys<'a>(rollups: impl IntoIterator<Item = &'a MinuteRollup>) -> BTreeSet<u64> {
    rollups
        .into_iter()
        .flat_map(|r| r.issues.keys().copied())
        .collect()
}

fn route_label(dict: &Dict, id: u64) -> Option<(String, String)> {
    if id == OTHER_ROUTE {
        return Some(("*".to_owned(), "Other routes".to_owned()));
    }
    match dict.get(id) {
        Some(DictEntry::Route { method, label, .. }) => Some((method.clone(), label.clone())),
        _ => None,
    }
}

fn route_display(dict: &Dict, id: u64) -> Option<String> {
    route_label(dict, id).map(|(m, l)| format!("{m} {l}"))
}

/// The point layout for a range: `(bucket width ms, bucket count)`.
fn layout(range: InsightsRange) -> (u64, u64) {
    match range {
        InsightsRange::FiveMinutes => (10_000, 30),
        InsightsRange::OneHour => (MINUTE_MS, 60),
        _ => (15 * MINUTE_MS, 96),
    }
}

/// Inputs to [`overview`] for one site.
pub struct OverviewInput<'a> {
    /// The site's minute rollups, oldest first: a fingerprint's route is the
    /// one from the last rollup that ran it.
    pub rollups: Vec<&'a MinuteRollup>,
    /// The site's in-memory recent requests.
    pub recent: Vec<&'a FinalRequest>,
    /// The site's dictionary.
    pub dict: &'a Dict,
    /// Dismissed issue keys.
    pub dismissed: &'a BTreeSet<u64>,
}

/// Compute the Overview for one site.
///
/// The 5m range is exact, from `recent`, in 30 points of 10 s; static,
/// dropped counts come from the rollups of the minutes it overlaps. 1h uses 60
/// minute points and 24h 96 points of 15 min, both from rollups. The p50 delta
/// (5m and 1h only) compares against the previous equal window taken from
/// minute rollups, because those requests may already be trimmed from
/// `recent`. That window ends at the minute holding the current window's
/// start, so no request is counted on both sides. `ext_available` is left
/// `true`: the caller knows the site's PHP version.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn overview(input: &OverviewInput<'_>, range: InsightsRange, now_ms: u64) -> InsightsOverview {
    let (width, count) = layout(range);
    let span = width * count;
    let start = (now_ms - now_ms % width + width).saturating_sub(span);
    let mut points: Vec<MinuteRollup> = (0..count)
        .map(|i| MinuteRollup::new(start + i * width))
        .collect();
    let mut total = MinuteRollup::new(start);
    let bucket = |ts: u64| -> Option<usize> {
        (ts >= start && ts < start + span)
            .then(|| usize::try_from((ts - start) / width).unwrap_or(0))
    };
    if range == InsightsRange::FiveMinutes {
        for r in input.recent.iter().filter(|r| bucket(r.ts_ms).is_some()) {
            total.fold_request(r);
            if let Some(p) = bucket(r.ts_ms).and_then(|i| points.get_mut(i)) {
                p.fold_request(r);
            }
        }
        total.dropped_frames = 0;
        let first_minute = minute_of(start);
        for m in input
            .rollups
            .iter()
            .filter(|m| m.minute >= first_minute && m.minute <= now_ms)
        {
            total.static_skipped += m.static_skipped;
            total.dropped_requests += m.dropped_requests;
            total.dropped_frames += m.dropped_frames;
        }
    } else {
        for m in &input.rollups {
            if let Some(i) = bucket(m.minute) {
                total.merge(m);
                if let Some(p) = points.get_mut(i) {
                    p.merge(m);
                }
            }
        }
    }

    let p50 = total.warm_hist.quantile(0.5).map(us_to_ms);
    let p95 = total.warm_hist.quantile(0.95).map(us_to_ms);
    let p50_delta_ms = if range == InsightsRange::OneDay {
        None
    } else {
        let cur_start = minute_of(start);
        let prev_start = cur_start.saturating_sub(range.millis());
        let mut prev = LatencyHist::new();
        for m in input
            .rollups
            .iter()
            .filter(|m| m.minute >= prev_start && m.minute < cur_start)
        {
            prev.merge(&m.warm_hist);
        }
        match (p50, prev.quantile(0.5)) {
            (Some(cur), Some(before)) => Some(cur - us_to_ms(before)),
            _ => None,
        }
    };

    let series: Vec<InsightsPoint> = points
        .iter()
        .enumerate()
        .map(|(i, p)| InsightsPoint {
            t_ms: start + u64::try_from(i).unwrap_or(0) * width,
            count: p.requests,
            p50_ms: p.warm_hist.quantile(0.5).map(us_to_ms),
            p95_ms: p.warm_hist.quantile(0.95).map(us_to_ms),
            errors: p.s5xx,
        })
        .collect();
    let spike = series
        .iter()
        .filter_map(|p| p.p95_ms.map(|v| (v, p.t_ms)))
        .fold(None, |best: Option<(f64, u64)>, cur| match best {
            Some(b) if b.0 >= cur.0 => Some(b),
            _ => Some(cur),
        });

    let minutes = (range.millis() / MINUTE_MS).max(1);
    let kpis = InsightsKpis {
        p50_ms: p50,
        p95_ms: p95,
        p50_delta_ms,
        p95_spike_ms: spike.map(|s| s.0),
        p95_spike_at_ms: spike.map(|s| s.1),
        requests: total.requests,
        per_minute: f(total.requests) / f(minutes),
        static_skipped: total.static_skipped,
        failed_pct: if total.requests == 0 {
            0.0
        } else {
            f(total.s5xx) * 100.0 / f(total.requests)
        },
        s5xx: total.s5xx,
        s4xx: total.s4xx,
        queries_avg: (total.ext_requests > 0)
            .then(|| f(total.queries_total) / f(total.ext_requests)),
        queries_max: total.max_queries.0,
        queries_max_route: (total.max_queries.0 > 0)
            .then(|| route_display(input.dict, total.max_queries.1))
            .flatten(),
    };

    InsightsOverview {
        range,
        kpis,
        cold: total.cold,
        dropped_requests: total.dropped_requests,
        dropped_frames: total.dropped_frames,
        ext_seen: total.ext_requests > 0,
        ext_available: true,
        series,
        breakdown: breakdown(&total),
        routes: routes(&total, input.dict),
        queries: queries(&total, input.dict),
        issues: issues(&total, input.dict, input.dismissed),
    }
}

fn breakdown(total: &MinuteRollup) -> Vec<InsightsSlice> {
    if total.breakdown_requests == 0 {
        return Vec::new();
    }
    let parts = [
        (InsightsTimePart::App, total.sum_app_us),
        (InsightsTimePart::Db, total.sum_db_us),
        (InsightsTimePart::Http, total.sum_http_us),
        (InsightsTimePart::Wait, total.sum_wait_us),
    ];
    let sum: u64 = parts.iter().map(|(_, v)| *v).sum();
    let n = f(total.breakdown_requests);
    parts
        .into_iter()
        .map(|(part, v)| InsightsSlice {
            part,
            share: if sum == 0 { 0.0 } else { f(v) / f(sum) },
            mean_ms: us_to_ms(v) / n,
        })
        .collect()
}

fn routes(total: &MinuteRollup, dict: &Dict) -> Vec<InsightsRoute> {
    let all_us: u64 = total.routes.values().map(|r| r.sum_us).sum();
    let mut rows: Vec<(&u64, &RouteAgg)> = total.routes.iter().collect();
    rows.sort_by(|a, b| b.1.sum_us.cmp(&a.1.sum_us).then(a.0.cmp(b.0)));
    rows.into_iter()
        .take(TOP_ROWS)
        .map(|(id, agg)| {
            let (method, label, kind, name, action) = if let Some(DictEntry::Route {
                method,
                label,
                kind,
                name,
                action,
            }) = dict.get(*id)
            {
                (
                    method.clone(),
                    label.clone(),
                    kind.to_ipc(),
                    name.clone(),
                    action.clone(),
                )
            } else {
                let (m, l) = route_label(dict, *id)
                    .unwrap_or_else(|| (String::new(), "(unknown route)".to_owned()));
                (m, l, InsightsRouteKind::Path, None, None)
            };
            InsightsRoute {
                method,
                label,
                kind,
                name,
                action,
                requests: agg.requests,
                p50_ms: agg.warm_hist.quantile(0.5).map(us_to_ms),
                p95_ms: agg.warm_hist.quantile(0.95).map(us_to_ms),
                share: if all_us == 0 {
                    0.0
                } else {
                    f(agg.sum_us) / f(all_us)
                },
                queries_per_request: (agg.ext_requests > 0)
                    .then(|| f(agg.queries) / f(agg.ext_requests)),
                issue_kinds: IssueKind::from_bits(agg.issue_kinds)
                    .into_iter()
                    .map(IssueKind::to_ipc)
                    .collect(),
            }
        })
        .collect()
}

fn queries(total: &MinuteRollup, dict: &Dict) -> Vec<InsightsQuery> {
    let mut rows: Vec<(&u64, &FpAgg)> = total.fingerprints.iter().collect();
    rows.sort_by(|a, b| b.1.sum_us.cmp(&a.1.sum_us).then(a.0.cmp(b.0)));
    rows.into_iter()
        .take(TOP_ROWS)
        .map(|(fp, agg)| InsightsQuery {
            fingerprint: match dict.get(*fp) {
                Some(DictEntry::Sql { fingerprint, .. }) => fingerprint.clone(),
                _ => "(unknown query)".to_owned(),
            },
            count: agg.count,
            total_ms: us_to_ms(agg.sum_us),
            mean_ms: if agg.count == 0 {
                0.0
            } else {
                us_to_ms(agg.sum_us) / f(agg.count)
            },
            route_label: route_display(dict, agg.route_id),
        })
        .collect()
}

fn issues(total: &MinuteRollup, dict: &Dict, dismissed: &BTreeSet<u64>) -> Vec<InsightsIssue> {
    let mut rows: Vec<(&u64, &IssueAgg)> = total.issues.iter().collect();
    rows.sort_by(|a, b| b.1.sum_us.cmp(&a.1.sum_us).then(a.0.cmp(b.0)));
    rows.into_iter()
        .filter_map(|(key, agg)| {
            let Some(DictEntry::Issue {
                kind,
                fp_hash,
                file,
                line,
                route_id,
            }) = dict.get(*key)
            else {
                return None;
            };
            let sql = fp_hash.and_then(|fp| match dict.get(fp) {
                Some(DictEntry::Sql { fingerprint, .. }) => Some(fingerprint.clone()),
                _ => None,
            });
            let route = route_label(dict, *route_id);
            Some(InsightsIssue {
                key: key_hex(*key),
                kind: kind.to_ipc(),
                sql,
                hint: hint(*kind).to_owned(),
                file: file.clone(),
                line: *line,
                route_method: route.as_ref().map(|r| r.0.clone()),
                route_label: route.map(|r| r.1),
                occurrences: agg.occurrences,
                affected_requests: agg.affected_requests,
                impact_ms: us_to_ms(agg.sum_us),
                max_in_request: agg.max_in_request,
                last_request_id: agg.last_request.as_ref().map(|l| l.0.clone()),
                last_request_ts_ms: agg.last_request.as_ref().map(|l| l.1),
                dismissed: dismissed.contains(key),
            })
        })
        .take(TOP_ISSUES)
        .collect()
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
    use crate::pure::model::tests::request;
    use crate::pure::model::{FpUse, RequestIssue};
    use crate::pure::route::RouteKind;

    const NOW: u64 = 1_700_000_000_000 - 1_700_000_000_000 % 3_600_000 + 30 * MINUTE_MS + 5_000;

    fn with_route(mut r: FinalRequest, route_id: u64) -> FinalRequest {
        r.route_id = route_id;
        r
    }

    fn rich(id: &str, ts: u64, dur: u64, route_id: u64) -> FinalRequest {
        let mut r = with_route(request(id, ts, dur), route_id);
        r.has_summary = true;
        r.queries = 3;
        r.db_us = Some(dur / 4);
        r.http_us = Some(0);
        r.wait_us = Some(0);
        r.app_us = Some(dur - dur / 4);
        r.fingerprints = vec![FpUse {
            fp: 77,
            count: 3,
            time_us: dur / 4,
        }];
        r.issues = vec![RequestIssue {
            kind: IssueKind::NPlusOne,
            key: 99,
            count: 3,
            time_us: dur / 4,
            fp_hash: Some(77),
            file: None,
            line: None,
        }];
        r
    }

    fn dict() -> Dict {
        let mut d = Dict::new();
        d.insert(
            1,
            DictEntry::Route {
                method: "GET".into(),
                label: "/a".into(),
                kind: RouteKind::Template,
                name: None,
                action: None,
            },
        );
        d.insert(
            77,
            DictEntry::Sql {
                fingerprint: "select ?".into(),
                sample_sql: "select 1".into(),
            },
        );
        d.insert(
            99,
            DictEntry::Issue {
                kind: IssueKind::NPlusOne,
                fp_hash: Some(77),
                file: Some("f.php".into()),
                line: Some(3),
                route_id: 1,
            },
        );
        d
    }

    #[test]
    fn merging_deltas_equals_a_single_fold() {
        let reqs: Vec<FinalRequest> = (0..10)
            .map(|i| rich(&i.to_string(), NOW, 1_000 * (i + 1), i % 3))
            .collect();
        let mut single = MinuteRollup::new(NOW);
        for r in &reqs {
            single.fold_request(r);
        }
        let mut a = MinuteRollup::new(NOW);
        let mut b = MinuteRollup::new(NOW);
        for (i, r) in reqs.iter().enumerate() {
            if i % 2 == 0 {
                a.fold_request(r);
            } else {
                b.fold_request(r);
            }
        }
        let mut merged = MinuteRollup::new(NOW);
        merged.merge(&a);
        merged.merge(&b);
        assert_eq!(merged.warm_hist, single.warm_hist);
        assert_eq!(merged.routes, single.routes);
        assert_eq!(
            merged.fingerprints.get(&77).map(|f| (f.count, f.sum_us)),
            single.fingerprints.get(&77).map(|f| (f.count, f.sum_us))
        );
        let strip = |mut m: MinuteRollup| {
            m.fingerprints.clear();
            m.issues.values_mut().for_each(|i| i.last_request = None);
            m
        };
        assert_eq!(strip(merged), strip(single));
    }

    #[test]
    fn route_overflow_folds_into_other() {
        let mut m = MinuteRollup::new(NOW);
        for i in 1..=(ROUTES_CAP as u64 + 5) {
            m.fold_request(&with_route(request("x", NOW, 1_000), i));
        }
        assert_eq!(m.routes.len(), ROUTES_CAP + 1);
        assert_eq!(m.routes.get(&OTHER_ROUTE).map(|r| r.requests), Some(5));
        let mut other = MinuteRollup::new(NOW);
        other.fold_request(&with_route(request("y", NOW, 1_000), 1_000));
        m.merge(&other);
        assert_eq!(m.routes.get(&OTHER_ROUTE).map(|r| r.requests), Some(6));
        assert_eq!(m.requests, ROUTES_CAP as u64 + 6);
    }

    #[test]
    fn other_route_does_not_take_a_named_slot_on_merge() {
        let mut source = MinuteRollup::new(NOW);
        for i in 1..=(ROUTES_CAP as u64 + 3) {
            source.fold_request(&with_route(request("x", NOW, 1_000), i));
        }
        assert_eq!(source.routes.len(), ROUTES_CAP + 1);
        let mut merged = MinuteRollup::new(NOW);
        merged.merge(&source);
        assert_eq!(merged.routes, source.routes);
        merged.merge(&source);
        assert_eq!(merged.routes.len(), ROUTES_CAP + 1);
        assert_eq!(merged.routes.get(&OTHER_ROUTE).map(|r| r.requests), Some(6));
        assert_eq!(merged.routes.get(&1).map(|r| r.requests), Some(2));
    }

    #[test]
    fn five_minute_dropped_frames_are_counted_once() {
        let d = Dict::new();
        let dismissed = BTreeSet::new();
        let cases: &[(u32, u64, u64)] = &[(2, 0, 2), (2, 3, 5), (0, 3, 3), (0, 0, 0)];
        for (per_request, orphans, want) in cases {
            let mut r = request("a", NOW, 1_000);
            r.dropped_frames = *per_request;
            let mut m = MinuteRollup::new(NOW);
            m.fold_request(&r);
            m.dropped_frames += orphans;
            let o = overview(
                &OverviewInput {
                    rollups: vec![&m],
                    recent: vec![&r],
                    dict: &d,
                    dismissed: &dismissed,
                },
                InsightsRange::FiveMinutes,
                NOW,
            );
            assert_eq!(o.dropped_frames, *want, "{per_request} {orphans}");
        }
    }

    #[test]
    fn five_minute_delta_excludes_the_overlapping_minute() {
        let d = Dict::new();
        let dismissed = BTreeSet::new();
        let start = NOW - NOW % 10_000 + 10_000 - 5 * MINUTE_MS;
        let cur_req = request("a", start + 20_000, 20_000);
        let mut cur = MinuteRollup::new(cur_req.ts_ms);
        cur.fold_request(&cur_req);
        let prev_ts = minute_of(start) - 1;
        let mut prev = MinuteRollup::new(prev_ts);
        prev.fold_request(&request("b", prev_ts, 10_000));
        let cases: Vec<(Vec<&MinuteRollup>, bool)> =
            vec![(vec![&cur], false), (vec![&prev, &cur], true)];
        for (rollups, want) in cases {
            let o = overview(
                &OverviewInput {
                    rollups,
                    recent: vec![&cur_req],
                    dict: &d,
                    dismissed: &dismissed,
                },
                InsightsRange::FiveMinutes,
                NOW,
            );
            assert_eq!(o.kpis.requests, 1);
            assert_eq!(o.kpis.p50_delta_ms.is_some(), want);
        }
    }

    #[test]
    fn cold_is_counted_but_not_in_the_histogram() {
        let mut m = MinuteRollup::new(NOW);
        let mut cold = request("c", NOW, 900_000);
        cold.cold = true;
        m.fold_request(&cold);
        m.fold_request(&request("w", NOW, 1_000));
        assert_eq!(m.requests, 2);
        assert_eq!(m.cold, 1);
        assert_eq!(m.warm_hist.count(), 1);
    }

    #[test]
    fn empty_range_gives_zeros() {
        let d = Dict::new();
        let dismissed = BTreeSet::new();
        for range in [
            InsightsRange::FiveMinutes,
            InsightsRange::OneHour,
            InsightsRange::OneDay,
        ] {
            let o = overview(
                &OverviewInput {
                    rollups: vec![],
                    recent: vec![],
                    dict: &d,
                    dismissed: &dismissed,
                },
                range,
                NOW,
            );
            assert_eq!(o.kpis.requests, 0);
            assert_eq!(o.kpis.p50_ms, None);
            assert_eq!(o.kpis.p50_delta_ms, None);
            assert!((o.kpis.failed_pct).abs() < f64::EPSILON);
            assert!(!o.ext_seen);
            assert!(o.breakdown.is_empty() && o.routes.is_empty() && o.issues.is_empty());
            assert!(o.series.iter().all(|p| p.count == 0));
        }
    }

    #[test]
    fn series_layouts_and_24h_edges() {
        let d = Dict::new();
        let dismissed = BTreeSet::new();
        let cases = [
            (InsightsRange::FiveMinutes, 30usize, 10_000u64),
            (InsightsRange::OneHour, 60, MINUTE_MS),
            (InsightsRange::OneDay, 96, 15 * MINUTE_MS),
        ];
        for (range, n, width) in cases {
            let o = overview(
                &OverviewInput {
                    rollups: vec![],
                    recent: vec![],
                    dict: &d,
                    dismissed: &dismissed,
                },
                range,
                NOW,
            );
            assert_eq!(o.series.len(), n);
            let last = o.series.last().unwrap();
            assert!(last.t_ms <= NOW && NOW < last.t_ms + width);
            assert_eq!(o.series[1].t_ms - o.series[0].t_ms, width);
        }
        let first_bucket = NOW - NOW % (15 * MINUTE_MS) + 15 * MINUTE_MS - 86_400_000;
        let mut inside = MinuteRollup::new(first_bucket);
        inside.fold_request(&request("in", first_bucket, 1_000));
        let mut outside = MinuteRollup::new(first_bucket - MINUTE_MS);
        outside.fold_request(&request("out", first_bucket - MINUTE_MS, 1_000));
        let mut current = MinuteRollup::new(NOW);
        current.fold_request(&request("now", NOW, 1_000));
        let o = overview(
            &OverviewInput {
                rollups: vec![&inside, &outside, &current],
                recent: vec![],
                dict: &d,
                dismissed: &dismissed,
            },
            InsightsRange::OneDay,
            NOW,
        );
        assert_eq!(o.kpis.requests, 2);
        assert_eq!(o.series[0].count, 1);
        assert_eq!(o.series[95].count, 1);
        assert_eq!(o.kpis.p50_delta_ms, None);
    }

    #[test]
    fn hour_delta_present_and_absent() {
        let d = Dict::new();
        let dismissed = BTreeSet::new();
        let mut cur = MinuteRollup::new(NOW);
        cur.fold_request(&request("a", NOW, 20_000));
        let prev_ts = NOW - 3_600_000;
        let mut prev = MinuteRollup::new(prev_ts);
        prev.fold_request(&request("b", prev_ts, 10_000));
        let with_prev = overview(
            &OverviewInput {
                rollups: vec![&cur, &prev],
                recent: vec![],
                dict: &d,
                dismissed: &dismissed,
            },
            InsightsRange::OneHour,
            NOW,
        );
        let delta = with_prev.kpis.p50_delta_ms.unwrap();
        assert!(delta > 5.0 && delta < 15.0, "{delta}");
        let without = overview(
            &OverviewInput {
                rollups: vec![&cur],
                recent: vec![],
                dict: &d,
                dismissed: &dismissed,
            },
            InsightsRange::OneHour,
            NOW,
        );
        assert_eq!(without.kpis.p50_delta_ms, None);
    }

    #[test]
    fn five_minute_delta_uses_rollups_not_recent() {
        let d = Dict::new();
        let dismissed = BTreeSet::new();
        let cur_req = request("a", NOW, 20_000);
        let mut cur = MinuteRollup::new(NOW);
        cur.fold_request(&cur_req);
        let prev_ts = NOW - 6 * MINUTE_MS;
        let mut prev = MinuteRollup::new(prev_ts);
        prev.fold_request(&request("b", prev_ts, 10_000));
        let o = overview(
            &OverviewInput {
                rollups: vec![&cur, &prev],
                recent: vec![&cur_req],
                dict: &d,
                dismissed: &dismissed,
            },
            InsightsRange::FiveMinutes,
            NOW,
        );
        assert_eq!(o.kpis.requests, 1);
        assert!(o.kpis.p50_delta_ms.is_some_and(|d| d > 5.0));
    }

    #[test]
    fn overview_resolves_labels_and_flags_dismissed() {
        let d = dict();
        let mut m = MinuteRollup::new(NOW);
        m.fold_request(&rich("a", NOW, 8_000, 1));
        m.static_skipped = 4;
        let dismissed = BTreeSet::from([99]);
        let o = overview(
            &OverviewInput {
                rollups: vec![&m],
                recent: vec![],
                dict: &d,
                dismissed: &dismissed,
            },
            InsightsRange::OneHour,
            NOW,
        );
        assert_eq!(o.kpis.static_skipped, 4);
        assert_eq!(o.kpis.queries_avg, Some(3.0));
        assert_eq!(o.kpis.queries_max, 3);
        assert_eq!(o.kpis.queries_max_route.as_deref(), Some("GET /a"));
        assert!(o.ext_seen);
        assert_eq!(o.routes[0].label, "/a");
        assert_eq!(o.routes[0].kind, InsightsRouteKind::Template);
        assert_eq!(o.queries[0].fingerprint, "select ?");
        assert_eq!(o.issues.len(), 1);
        assert!(o.issues[0].dismissed);
        assert_eq!(o.issues[0].sql.as_deref(), Some("select ?"));
        assert_eq!(o.issues[0].route_label.as_deref(), Some("/a"));
        assert_eq!(o.issues[0].key, "0000000000000063");
        assert_eq!(o.breakdown.len(), 4);
        let db = o
            .breakdown
            .iter()
            .find(|s| s.part == InsightsTimePart::Db)
            .unwrap();
        assert!((db.share - 0.25).abs() < 1e-9);
        assert!((db.mean_ms - 2.0).abs() < 1e-9);
    }

    #[test]
    fn retain_closure_keeps_issue_and_max_query_refs() {
        let mut d = dict();
        d.insert(
            2,
            DictEntry::Route {
                method: "GET".into(),
                label: "/b".into(),
                kind: RouteKind::Path,
                name: None,
                action: None,
            },
        );
        d.insert(
            555,
            DictEntry::Sql {
                fingerprint: "gone".into(),
                sample_sql: "gone".into(),
            },
        );
        let mut m = MinuteRollup::new(NOW);
        m.max_queries = (9, 2);
        m.issues.insert(99, IssueAgg::default());
        let keys = referenced_keys([&m], &d);
        assert!(keys.contains(&99));
        assert!(keys.contains(&77), "issue fingerprint kept");
        assert!(keys.contains(&1), "issue route kept");
        assert!(keys.contains(&2), "max-queries route kept");
        d.retain(&keys);
        assert!(d.contains(77) && d.contains(1) && d.contains(2) && d.contains(99));
        assert!(!d.contains(555));
    }

    #[test]
    fn open_issue_keys_union() {
        let mut a = MinuteRollup::new(NOW);
        a.issues.insert(1, IssueAgg::default());
        let mut b = MinuteRollup::new(NOW);
        b.issues.insert(2, IssueAgg::default());
        assert_eq!(open_issue_keys([&a, &b]), BTreeSet::from([1, 2]));
    }
}
