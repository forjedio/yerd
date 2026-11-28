//! The Insights aggregator: joins proxy samples with extension frames,
//! finalises requests, folds them into rollups and hands out flush batches.
//!
//! Pure and clock-free: every method that depends on time takes `now` in Unix
//! epoch milliseconds.
//!
//! ## Joining (contract C6)
//!
//! An untraced sample (including every proxy error) is finalised at once as
//! timing-only. A traced sample waits in `pending` for the extension's
//! summary for up to [`GRACE_MS`], or only [`PROBE_GRACE_MS`] while its PHP
//! version has sent no v2 summary since boot and no frame has arrived for the
//! request, so an old or missing extension does not delay timing-only
//! requests by the full grace. When the summary arrives with fewer frames
//! received than it announced, the request settles for up to [`SETTLE_MS`] so
//! frames still in flight are counted. Frames for an id that never gets a
//! sample are discarded as orphans after [`ORPHAN_MS`] of silence. Late frames
//! for a recently finalised id are discarded without being counted again.
//!
//! A missing summary only counts as dropped frames once a v2 summary has been
//! seen from that PHP version since boot, so an old or missing extension never
//! reports phantom losses.
//!
//! ## Flushing
//!
//! Finalised requests stay in `recent` until they are both flushed and older
//! than [`RECENT_MS`]. A flush batch is taken every [`FLUSH_INTERVAL_MS`], when
//! a site's unflushed bytes reach [`FLUSH_BYTES`], or when forced. A failed
//! write counts the batch's requests as dropped and marks them flushed so they
//! still trim; their rollup deltas stay in memory but are missing on disk.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::Arc;

use serde_json::Value;
use yerd_ipc::{DumpCategory, InsightsOverview, InsightsRange, InsightsSort};

use crate::error::InsightsError;
use crate::pure::contract::{ExtFrame, ExtSummary, ProxySample};
use crate::pure::dict::{Dict, DictEntry, SAMPLE_SQL_MAX};
use crate::pure::issues::{detect, QueryEvent, Thresholds};
use crate::pure::model::{DetailEvent, FinalRequest, FpUse, RequestDetail, RequestIssue};
use crate::pure::num::round_u64;
use crate::pure::retention::{hour_of, HOUR_MS, RETENTION_MS};
use crate::pure::rollup::{
    minute_of, open_issue_keys, overview, referenced_keys, MinuteRollup, OverviewInput,
};
use crate::pure::route::{route_key, truncate_utf8};
use crate::pure::sql::fingerprint;

/// How long a traced sample waits for its summary.
pub const GRACE_MS: u64 = 10_000;
/// How long a traced sample waits when nothing suggests the extension is
/// speaking protocol 2: no v2 summary from its PHP version since boot and no
/// frame for the request yet.
pub const PROBE_GRACE_MS: u64 = 1_500;
/// How long a summary waits for frames still in flight.
pub const SETTLE_MS: u64 = 1_000;
/// Silence after which frames without a sample are discarded.
pub const ORPHAN_MS: u64 = 30_000;
/// How long flushed requests stay in memory.
pub const RECENT_MS: u64 = 300_000;
/// Time-based flush trigger.
pub const FLUSH_INTERVAL_MS: u64 = 300_000;
/// Size-based flush trigger, per site.
pub const FLUSH_BYTES: usize = 8 * 1024 * 1024;
/// Most events kept per request.
pub const MAX_EVENTS_PER_REQUEST: usize = 1000;
/// Global cap on frame bytes held while joining.
pub const PENDING_BYTES_CAP: usize = 64 * 1024 * 1024;
/// How many finalised ids are remembered to discard late frames.
pub const RECENTLY_FINALISED_CAP: usize = 4096;
/// Most minute rollups kept per site.
pub const MAX_ROLLUPS: usize = 1440;
/// Most bytes kept of a SQL string in a detail.
pub const DETAIL_SQL_MAX: usize = 8 * 1024;
/// Most bytes kept of a dump's `value_html` in a detail.
pub const DETAIL_HTML_MAX: usize = 64 * 1024;
/// Most bytes kept of any other string in a detail.
pub const DETAIL_STRING_MAX: usize = 8 * 1024;
/// Most bytes of events kept in one detail before it is truncated.
pub const DETAIL_MAX: usize = 1024 * 1024;
/// Most rows a request list returns.
pub const LIST_LIMIT_MAX: u32 = 500;
/// Approximate in-memory size of a summary, for the flush trigger.
const SUMMARY_BYTES: usize = 300;

#[derive(Debug)]
struct Pending {
    sample: Option<ProxySample>,
    sample_at_ms: u64,
    frame_site: String,
    frames: Vec<ExtFrame>,
    summary: Option<ExtSummary>,
    summary_at_ms: u64,
    received: u32,
    cap_dropped: u32,
    max_seq: u32,
    last_seen_ms: u64,
    bytes: usize,
}

impl Pending {
    fn new(now: u64) -> Self {
        Self {
            sample: None,
            sample_at_ms: now,
            frame_site: String::new(),
            frames: Vec::new(),
            summary: None,
            summary_at_ms: now,
            received: 0,
            cap_dropped: 0,
            max_seq: 0,
            last_seen_ms: now,
            bytes: 0,
        }
    }

    /// Every frame the extension wrote has arrived. Frames it built but never
    /// wrote (`frames_unsent`) cannot arrive, so they are not waited for; they
    /// still count as dropped when the request is finalised.
    fn settled(&self) -> bool {
        self.summary
            .as_ref()
            .is_some_and(|s| self.received >= s.frames.saturating_sub(s.frames_unsent))
    }
}

#[derive(Debug, Clone)]
struct RecentEntry {
    req: FinalRequest,
    detail: Option<Arc<RequestDetail>>,
    flushed: bool,
    taken: bool,
}

#[derive(Debug, Default)]
struct SiteState {
    rollups: VecDeque<MinuteRollup>,
    unflushed_rollups: BTreeMap<u64, MinuteRollup>,
    dict: Dict,
    recent: VecDeque<RecentEntry>,
    unflushed_bytes: usize,
    last_flush_ms: u64,
    generation: u64,
    dismissed: BTreeSet<u64>,
    open_issues: BTreeSet<u64>,
}

impl SiteState {
    fn new(now: u64, generation: u64) -> Self {
        Self {
            last_flush_ms: now,
            generation,
            ..Self::default()
        }
    }

    fn is_empty(&self) -> bool {
        self.rollups.is_empty()
            && self.unflushed_rollups.is_empty()
            && self.dict.is_empty()
            && self.recent.is_empty()
            && self.dismissed.is_empty()
            && self.open_issues.is_empty()
    }

    fn rollup_mut(&mut self, ms: u64) -> Option<&mut MinuteRollup> {
        let minute = minute_of(ms);
        let pos = self
            .rollups
            .iter()
            .rposition(|r| r.minute <= minute)
            .map_or(0, |p| p + 1);
        let exists = pos > 0
            && self
                .rollups
                .get(pos - 1)
                .is_some_and(|r| r.minute == minute);
        if exists {
            return self.rollups.get_mut(pos - 1);
        }
        self.rollups.insert(pos, MinuteRollup::new(minute));
        self.rollups.get_mut(pos)
    }

    fn apply(&mut self, ms: u64, f: impl Fn(&mut MinuteRollup)) {
        if let Some(r) = self.rollup_mut(ms) {
            f(r);
        }
        let minute = minute_of(ms);
        f(self
            .unflushed_rollups
            .entry(minute)
            .or_insert_with(|| MinuteRollup::new(minute)));
    }
}

/// A batch of one site's unflushed data, grouped by UTC hour.
#[derive(Debug, Clone)]
pub struct FlushBatch {
    /// Site name.
    pub site: String,
    /// The site generation when taken; confirm and failure reports carry it
    /// back so a clear in between turns them into no-ops.
    pub generation: u64,
    /// Data per hour index.
    pub per_hour: BTreeMap<u64, HourBatch>,
}

impl FlushBatch {
    /// Every request id in the batch.
    #[must_use]
    pub fn ids(&self) -> Vec<String> {
        self.per_hour
            .values()
            .flat_map(|h| h.requests.iter().map(|(id, _, _)| id.clone()))
            .collect()
    }

    /// Every hour in the batch.
    #[must_use]
    pub fn hours(&self) -> Vec<u64> {
        self.per_hour.keys().copied().collect()
    }
}

/// One hour of a [`FlushBatch`].
#[derive(Debug, Clone, Default)]
pub struct HourBatch {
    /// Rollup deltas whose minute falls in this hour.
    pub rollup_deltas: Vec<MinuteRollup>,
    /// Dictionary entries not yet written for this hour; write them before the
    /// deltas.
    pub dict_entries: Vec<(u64, DictEntry)>,
    /// Requests whose start falls in this hour, with their detail.
    pub requests: Vec<(String, FinalRequest, Option<Arc<RequestDetail>>)>,
}

/// What a [`Aggregator::tick`] changed that the caller must persist.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TickOutcome {
    /// Sites whose dismissed set shrank during a rollup prune.
    pub dismissed_changed: Vec<String>,
}

/// The Insights aggregator. See the module docs.
#[derive(Debug, Default)]
pub struct Aggregator {
    sites: BTreeMap<String, SiteState>,
    pending: HashMap<String, Pending>,
    pending_bytes: usize,
    v2_seen: BTreeSet<String>,
    recently_finalised: HashSet<String>,
    finalised_order: VecDeque<String>,
    thresholds: Thresholds,
}

fn approx_size(v: &Value) -> usize {
    match v {
        Value::Null | Value::Bool(_) | Value::Number(_) => 8,
        Value::String(s) => s.len() + 2,
        Value::Array(a) => 2 + a.iter().map(approx_size).sum::<usize>(),
        Value::Object(o) => {
            2 + o
                .iter()
                .map(|(k, v)| k.len() + 3 + approx_size(v))
                .sum::<usize>()
        }
    }
}

fn cap_strings(v: &mut Value, key: Option<&str>) {
    match v {
        Value::String(s) => {
            let max = match key {
                Some("sql" | "sql_full") => DETAIL_SQL_MAX,
                Some("value_html") => DETAIL_HTML_MAX,
                _ => DETAIL_STRING_MAX,
            };
            truncate_utf8(s, max);
        }
        Value::Array(a) => a.iter_mut().for_each(|x| cap_strings(x, key)),
        Value::Object(o) => o
            .iter_mut()
            .for_each(|(k, x)| cap_strings(x, Some(k.as_str()))),
        _ => {}
    }
}

fn ms_field_us(payload: &Value, key: &str) -> Option<u64> {
    payload
        .get(key)
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite() && *v >= 0.0)
        .map(|v| round_u64(v * 1000.0))
}

fn str_of<'a>(payload: &'a Value, key: &str) -> Option<&'a str> {
    payload.get(key).and_then(Value::as_str)
}

impl Aggregator {
    /// An empty aggregator with default thresholds.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The thresholds applied to newly finalised requests.
    #[must_use]
    pub fn thresholds(&self) -> Thresholds {
        self.thresholds
    }

    /// Replace the thresholds; requests already finalised keep their issues.
    pub fn set_thresholds(&mut self, t: Thresholds) {
        self.thresholds = t;
    }

    fn site_mut(&mut self, site: &str, now: u64) -> &mut SiteState {
        self.sites
            .entry(site.to_owned())
            .or_insert_with(|| SiteState::new(now, 0))
    }

    fn remember_finalised(&mut self, id: &str) {
        if self.recently_finalised.insert(id.to_owned()) {
            self.finalised_order.push_back(id.to_owned());
        }
        while self.finalised_order.len() > RECENTLY_FINALISED_CAP {
            if let Some(old) = self.finalised_order.pop_front() {
                self.recently_finalised.remove(&old);
            }
        }
    }

    fn remove_pending(&mut self, id: &str) -> Option<Pending> {
        let p = self.pending.remove(id)?;
        self.pending_bytes = self.pending_bytes.saturating_sub(p.bytes);
        Some(p)
    }

    /// Ingest a proxy sample.
    pub fn on_sample(&mut self, s: ProxySample, now: u64) {
        if !s.traced {
            self.finalise(s, None, now);
            return;
        }
        if self.recently_finalised.contains(&s.id) {
            return;
        }
        let id = s.id.clone();
        let entry = self
            .pending
            .entry(id.clone())
            .or_insert_with(|| Pending::new(now));
        entry.sample_at_ms = now;
        entry.sample = Some(s);
        if entry.summary.is_some() {
            if let Some(php) = entry.sample.as_ref().map(|s| s.php.clone()) {
                self.v2_seen.insert(php);
            }
        }
        if entry.settled() {
            self.finalise_pending(&id, now);
        }
    }

    /// Ingest one protocol-2 extension frame.
    pub fn on_frame(&mut self, f: ExtFrame, now: u64) {
        if self.recently_finalised.contains(&f.request_id) {
            return;
        }
        let id = f.request_id.clone();
        let size = approx_size(&f.payload) + 64;
        let room = self.pending_bytes.saturating_add(size) <= PENDING_BYTES_CAP;
        let p = self
            .pending
            .entry(id.clone())
            .or_insert_with(|| Pending::new(now));
        p.received = p.received.saturating_add(1);
        p.last_seen_ms = now;
        if p.frame_site.is_empty() {
            p.frame_site.clone_from(&f.site);
        }
        if let Some(seq) = f.seq {
            p.max_seq = p.max_seq.max(seq);
        }
        let mut added = 0;
        if f.category == DumpCategory::Request {
            if p.summary.is_none() {
                if let Some(summary) = ExtSummary::from_payload(&f.payload) {
                    p.summary = Some(summary);
                    p.summary_at_ms = now;
                }
            }
        } else if p.frames.len() >= MAX_EVENTS_PER_REQUEST || !room {
            p.cap_dropped = p.cap_dropped.saturating_add(1);
        } else {
            p.frames.push(f);
            p.bytes += size;
            added = size;
        }
        let php = p
            .summary
            .as_ref()
            .and(p.sample.as_ref())
            .map(|s| s.php.clone());
        let ready = p.sample.is_some() && p.settled();
        self.pending_bytes = self.pending_bytes.saturating_add(added);
        if let Some(php) = php {
            self.v2_seen.insert(php);
        }
        if ready {
            self.finalise_pending(&id, now);
        }
    }

    fn finalise_pending(&mut self, id: &str, now: u64) {
        let Some(p) = self.remove_pending(id) else {
            return;
        };
        let Some(sample) = p.sample.clone() else {
            return;
        };
        self.finalise(sample, Some(p), now);
    }

    /// Count `n` proxy samples lost for `site` (channel full).
    pub fn note_proxy_dropped(&mut self, site: &str, n: u64, now: u64) {
        if n == 0 {
            return;
        }
        self.site_mut(site, now)
            .apply(now, |r| r.dropped_requests += n);
    }

    /// Count one static file served without PHP for `site`.
    pub fn note_static(&mut self, site: &str, ts_ms: u64, now: u64) {
        self.site_mut(site, now)
            .apply(ts_ms, |r| r.static_skipped += 1);
    }

    fn grace_for(&self, sample: &ProxySample, p: &Pending) -> u64 {
        if p.received == 0 && !self.v2_seen.contains(&sample.php) {
            PROBE_GRACE_MS
        } else {
            GRACE_MS
        }
    }

    /// Advance timers: finalise traced samples past their grace or settle
    /// window, discard orphans, trim `recent`, and prune rollups once the
    /// oldest minute is past 24h.
    pub fn tick(&mut self, now: u64) -> TickOutcome {
        let mut finalise: Vec<String> = Vec::new();
        let mut orphans: Vec<String> = Vec::new();
        for (id, p) in &self.pending {
            match (&p.sample, &p.summary) {
                (Some(s), None) if now.saturating_sub(p.sample_at_ms) >= self.grace_for(s, p) => {
                    finalise.push(id.clone());
                }
                (Some(_), Some(_)) if now.saturating_sub(p.summary_at_ms) >= SETTLE_MS => {
                    finalise.push(id.clone());
                }
                (None, _) if now.saturating_sub(p.last_seen_ms) >= ORPHAN_MS => {
                    orphans.push(id.clone());
                }
                _ => {}
            }
        }
        for id in finalise {
            self.finalise_pending(&id, now);
        }
        for id in orphans {
            if let Some(p) = self.remove_pending(&id) {
                let lost = p
                    .summary
                    .as_ref()
                    .map_or(p.received, |s| p.received.max(s.frames));
                self.remember_finalised(&id);
                if !p.frame_site.is_empty() {
                    self.site_mut(&p.frame_site, now)
                        .apply(now, |r| r.dropped_frames += u64::from(lost));
                }
            }
        }
        let mut outcome = TickOutcome::default();
        for (name, site) in &mut self.sites {
            site.recent
                .retain(|e| !(e.flushed && e.req.ts_ms.saturating_add(RECENT_MS) < now));
            let expired = |r: &MinuteRollup| r.minute.saturating_add(RETENTION_MS) <= now;
            if site.rollups.front().is_some_and(expired) || site.rollups.len() > MAX_ROLLUPS {
                while site.rollups.front().is_some_and(expired) || site.rollups.len() > MAX_ROLLUPS
                {
                    site.rollups.pop_front();
                }
                let keys = referenced_keys(site.rollups.iter(), &site.dict);
                site.dict.retain(&keys);
                site.open_issues = open_issue_keys(site.rollups.iter());
                let before = site.dismissed.len();
                let dict = &site.dict;
                site.dismissed.retain(|k| dict.contains(*k));
                if site.dismissed.len() != before {
                    outcome.dismissed_changed.push(name.clone());
                }
            }
        }
        outcome
    }

    #[allow(clippy::too_many_lines)]
    fn finalise(&mut self, s: ProxySample, p: Option<Pending>, now: u64) {
        let summary = p.as_ref().and_then(|p| p.summary.clone());
        let count_missing = summary.is_some() || self.v2_seen.contains(&s.php);
        let dropped_frames = match &p {
            Some(p) => {
                let missing = match &summary {
                    Some(sm) => sm.frames.saturating_sub(p.received),
                    None if count_missing && s.traced => {
                        (p.max_seq.saturating_add(1).saturating_sub(p.received)).max(1)
                    }
                    None => 0,
                };
                missing.saturating_add(p.cap_dropped)
            }
            None => 0,
        };
        if summary.is_some() {
            self.v2_seen.insert(s.php.clone());
        }
        let cap_dropped = p.as_ref().is_some_and(|p| p.cap_dropped > 0);
        let mut frames = p.map(|p| p.frames).unwrap_or_default();
        frames.sort_by_key(|f| (f.t_us.unwrap_or(u64::MAX), f.seq.unwrap_or(u32::MAX)));

        let route = route_key(
            &s.method,
            &s.path,
            s.script.as_deref(),
            summary.as_ref().and_then(|sm| sm.route.as_ref()),
        );
        let route_id = route.id();

        let mut events: Vec<DetailEvent> = Vec::with_capacity(frames.len());
        let mut queries: Vec<QueryEvent> = Vec::new();
        let mut fp_uses: BTreeMap<u64, FpUse> = BTreeMap::new();
        let mut sql_entries: Vec<(u64, DictEntry)> = Vec::new();
        let (mut db_us, mut http_us, mut dumps, mut logs) = (0u64, 0u64, 0u32, 0u32);
        let mut detail_bytes = 0usize;
        let mut truncated = false;
        for (i, f) in frames.into_iter().enumerate() {
            let idx = u32::try_from(i).unwrap_or(u32::MAX);
            let mut dur_us = None;
            match f.category {
                DumpCategory::Query => {
                    let time_us = ms_field_us(&f.payload, "time_ms").unwrap_or(0);
                    db_us = db_us.saturating_add(time_us);
                    dur_us = Some(time_us);
                    let sql = str_of(&f.payload, "sql").unwrap_or("");
                    let full = str_of(&f.payload, "sql_full").unwrap_or("");
                    let fp = fingerprint(sql);
                    let q = QueryEvent::new(
                        idx,
                        sql,
                        &fp,
                        full,
                        f.payload.get("bindings").unwrap_or(&Value::Null),
                        time_us,
                        f.payload.get("row_count").and_then(Value::as_u64),
                        str_of(&f.payload, "file").map(str::to_owned),
                        f.payload
                            .get("line")
                            .and_then(Value::as_u64)
                            .and_then(|l| u32::try_from(l).ok()),
                    );
                    let u = fp_uses.entry(q.fp_hash).or_insert(FpUse {
                        fp: q.fp_hash,
                        count: 0,
                        time_us: 0,
                    });
                    u.count = u.count.saturating_add(1);
                    u.time_us = u.time_us.saturating_add(time_us);
                    if !sql_entries.iter().any(|(k, _)| *k == q.fp_hash) {
                        let mut sample_sql = if full.is_empty() { sql } else { full }.to_owned();
                        truncate_utf8(&mut sample_sql, SAMPLE_SQL_MAX);
                        let mut fingerprint = fp;
                        truncate_utf8(&mut fingerprint, DETAIL_SQL_MAX);
                        sql_entries.push((
                            q.fp_hash,
                            DictEntry::Sql {
                                fingerprint,
                                sample_sql,
                            },
                        ));
                    }
                    queries.push(q);
                }
                DumpCategory::Http => {
                    let us = ms_field_us(&f.payload, "duration_ms").unwrap_or(0);
                    http_us = http_us.saturating_add(us);
                    dur_us = Some(us);
                }
                DumpCategory::Dump => dumps = dumps.saturating_add(1),
                DumpCategory::Log => logs = logs.saturating_add(1),
                _ => {}
            }
            if truncated {
                continue;
            }
            let mut payload = f.payload;
            cap_strings(&mut payload, None);
            let size = approx_size(&payload) + 32;
            if detail_bytes + size > DETAIL_MAX {
                truncated = true;
                continue;
            }
            detail_bytes += size;
            events.push(DetailEvent {
                idx,
                category: f.category,
                t_us: f.t_us,
                dur_us,
                payload,
            });
        }

        let hits = detect(&queries, &self.thresholds);
        let has_summary = summary.is_some();
        let wait_us = summary
            .as_ref()
            .map(|sm| sm.start_us.saturating_sub(s.start_us));
        let app_us = wait_us.map(|w| {
            s.duration_us
                .saturating_sub(w)
                .saturating_sub(db_us)
                .saturating_sub(http_us)
        });
        let mut content_type = s.content_type.clone();
        if let Some(ct) = content_type.as_mut() {
            truncate_utf8(ct, 64);
        }
        let req = FinalRequest {
            id: s.id.clone(),
            site: s.site.clone(),
            php: s.php.clone(),
            ts_ms: s.start_us / 1000,
            method: route.method.clone(),
            path: s.path.clone(),
            route_id,
            route_label: route.label.clone(),
            route_kind: route.kind,
            route_name: route.name.clone(),
            route_action: route.action.clone(),
            status: s.status,
            duration_us: s.duration_us,
            ext_duration_us: summary.as_ref().map(|sm| sm.duration_us),
            bytes: s.bytes,
            content_type,
            cold: summary.as_ref().is_some_and(|sm| sm.worker_requests == 1),
            aborted: s.aborted,
            proxy_error: s.proxy_error,
            traced: s.traced,
            has_summary,
            has_detail: !events.is_empty(),
            queries: u32::try_from(queries.len()).unwrap_or(u32::MAX),
            dumps,
            logs,
            dropped_frames,
            db_us: has_summary.then_some(db_us),
            http_us: has_summary.then_some(http_us),
            wait_us,
            app_us,
            fingerprints: fp_uses.into_values().collect(),
            issues: hits.iter().map(RequestIssue::from).collect(),
        };
        let detail = (!events.is_empty()).then(|| {
            Arc::new(RequestDetail {
                id: s.id.clone(),
                events,
                issues: hits.clone(),
                truncated: truncated || cap_dropped,
            })
        });

        self.remember_finalised(&s.id);
        let site = self.site_mut(&s.site, now);
        site.dict.insert(
            route_id,
            DictEntry::Route {
                method: route.method,
                label: route.label,
                kind: route.kind,
                name: route.name,
                action: route.action,
            },
        );
        for (k, e) in sql_entries {
            site.dict.insert(k, e);
        }
        for h in &hits {
            site.dict.insert(
                h.key,
                DictEntry::Issue {
                    kind: h.kind,
                    fp_hash: h.fp_hash,
                    file: h.file.clone(),
                    line: h.line,
                    route_id,
                },
            );
            site.open_issues.insert(h.key);
        }
        site.apply(req.ts_ms, |r| r.fold_request(&req));
        site.unflushed_bytes = site
            .unflushed_bytes
            .saturating_add(SUMMARY_BYTES + detail_bytes);
        site.recent.push_back(RecentEntry {
            req,
            detail,
            flushed: false,
            taken: false,
        });
    }

    /// Take flush batches: for every site (or only `site`) when `force`, when
    /// [`FLUSH_INTERVAL_MS`] passed since its last flush, or when its
    /// unflushed bytes reached [`FLUSH_BYTES`]. Sites with nothing new give no
    /// batch.
    pub fn take_flush(&mut self, now: u64, force: bool, site: Option<&str>) -> Vec<FlushBatch> {
        let mut out = Vec::new();
        for (name, st) in &mut self.sites {
            if site.is_some_and(|only| only != name.as_str()) {
                continue;
            }
            let due = force
                || now.saturating_sub(st.last_flush_ms) >= FLUSH_INTERVAL_MS
                || st.unflushed_bytes >= FLUSH_BYTES;
            if !due {
                continue;
            }
            st.last_flush_ms = now;
            st.unflushed_bytes = 0;
            let mut per_hour: BTreeMap<u64, HourBatch> = BTreeMap::new();
            for (_, delta) in std::mem::take(&mut st.unflushed_rollups) {
                per_hour
                    .entry(hour_of(delta.minute))
                    .or_default()
                    .rollup_deltas
                    .push(delta);
            }
            for e in st.recent.iter_mut().filter(|e| !e.flushed && !e.taken) {
                e.taken = true;
                per_hour
                    .entry(hour_of(e.req.ts_ms))
                    .or_default()
                    .requests
                    .push((e.req.id.clone(), e.req.clone(), e.detail.clone()));
            }
            if per_hour.is_empty() {
                continue;
            }
            for (hour, batch) in &mut per_hour {
                let keys = referenced_keys(batch.rollup_deltas.iter(), &st.dict);
                batch.dict_entries = st.dict.take_unwritten(*hour, keys);
            }
            out.push(FlushBatch {
                site: name.clone(),
                generation: st.generation,
                per_hour,
            });
        }
        out
    }

    /// A batch was written: mark its requests flushed and drop their details
    /// from memory. Ignored when the site's generation changed since.
    pub fn confirm_flush(&mut self, site: &str, generation: u64, ids: &[String]) {
        let Some(st) = self.sites.get_mut(site) else {
            return;
        };
        if st.generation != generation {
            return;
        }
        let ids: HashSet<&str> = ids.iter().map(String::as_str).collect();
        for e in st
            .recent
            .iter_mut()
            .filter(|e| ids.contains(e.req.id.as_str()))
        {
            e.flushed = true;
            e.taken = false;
            e.detail = None;
        }
    }

    /// A batch failed to write: count its requests as dropped, mark them
    /// flushed (listable without detail, so they still trim) and forget the
    /// written dictionary keys of its hours. Ignored when the generation
    /// changed since.
    pub fn flush_failed(
        &mut self,
        site: &str,
        generation: u64,
        ids: &[String],
        hours: &[u64],
        now: u64,
    ) {
        let Some(st) = self.sites.get_mut(site) else {
            return;
        };
        if st.generation != generation {
            return;
        }
        let n = u64::try_from(ids.len()).unwrap_or(u64::MAX);
        if n > 0 {
            st.apply(now, |r| r.dropped_requests += n);
        }
        let ids: HashSet<&str> = ids.iter().map(String::as_str).collect();
        for e in st
            .recent
            .iter_mut()
            .filter(|e| ids.contains(e.req.id.as_str()))
        {
            e.flushed = true;
            e.taken = false;
            e.detail = None;
        }
        for h in hours {
            st.dict.forget_hour(*h);
        }
    }

    /// The store pruned these hours for `site`.
    pub fn on_pruned(&mut self, site: &str, hours: &[u64]) {
        if let Some(st) = self.sites.get_mut(site) {
            for h in hours {
                st.dict.forget_hour(*h);
            }
        }
    }

    /// Reset everything held for `site`, its pending joins included, and bump
    /// its generation.
    pub fn clear(&mut self, site: &str, now: u64) {
        let generation = self.sites.get(site).map_or(0, |s| s.generation) + 1;
        self.sites
            .insert(site.to_owned(), SiteState::new(now, generation));
        let ids: Vec<String> = self
            .pending
            .iter()
            .filter(|(_, p)| match &p.sample {
                Some(s) => s.site == site,
                None => p.frame_site == site,
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.remove_pending(&id);
        }
    }

    /// The Overview for `site`. An unknown site gives an empty overview.
    #[must_use]
    pub fn overview(&self, site: &str, range: InsightsRange, now: u64) -> InsightsOverview {
        let Some(st) = self.sites.get(site) else {
            return InsightsOverview::empty(range);
        };
        overview(
            &OverviewInput {
                rollups: st.rollups.iter().collect(),
                recent: st.recent.iter().map(|e| &e.req).collect(),
                dict: &st.dict,
                dismissed: &st.dismissed,
            },
            range,
            now,
        )
    }

    /// In-memory requests for `site` within `range`, filtered, sorted and
    /// limited (at most [`LIST_LIMIT_MAX`]).
    #[must_use]
    pub fn recent_requests(
        &self,
        site: &str,
        range: InsightsRange,
        query: Option<&str>,
        sort: InsightsSort,
        limit: u32,
        now: u64,
    ) -> Vec<FinalRequest> {
        let Some(st) = self.sites.get(site) else {
            return Vec::new();
        };
        let since = now.saturating_sub(range.millis());
        select_requests(
            st.recent
                .iter()
                .map(|e| &e.req)
                .filter(|r| r.ts_ms >= since),
            query,
            sort,
            limit,
        )
    }

    /// The in-memory drill-in for a request. `None` when the request is not in
    /// memory, or its detail was already flushed to disk.
    #[must_use]
    pub fn recent_detail(&self, site: &str, id: &str) -> Option<yerd_ipc::InsightsRequestDetail> {
        let st = self.sites.get(site)?;
        let e = st.recent.iter().find(|e| e.req.id == id)?;
        match &e.detail {
            Some(d) => Some(d.to_ipc(&e.req)),
            None if !e.req.has_detail => Some(RequestDetail::empty(id).to_ipc(&e.req)),
            None => None,
        }
    }

    /// Dismiss or restore an issue. Returns whether anything changed. Never
    /// creates site state.
    pub fn dismiss(&mut self, site: &str, key: u64, dismissed: bool) -> bool {
        let Some(st) = self.sites.get_mut(site) else {
            return false;
        };
        if dismissed {
            st.dismissed.insert(key)
        } else {
            st.dismissed.remove(&key)
        }
    }

    /// The dismissed keys of `site` still present in its dictionary, for
    /// persisting.
    #[must_use]
    pub fn dismissed_keys(&self, site: &str) -> Vec<u64> {
        self.sites.get(site).map_or_else(Vec::new, |st| {
            st.dismissed
                .iter()
                .copied()
                .filter(|k| st.dict.contains(*k))
                .collect()
        })
    }

    /// Open (undismissed) issues of `site` within the retained rollups.
    #[must_use]
    pub fn open_issue_count(&self, site: &str) -> u32 {
        self.sites.get(site).map_or(0, |st| {
            u32::try_from(st.open_issues.difference(&st.dismissed).count()).unwrap_or(u32::MAX)
        })
    }

    /// The site's generation, bumped by [`Self::clear`]. 0 for an unknown
    /// site.
    #[must_use]
    pub fn generation(&self, site: &str) -> u64 {
        self.sites.get(site).map_or(0, |s| s.generation)
    }

    /// Names of the sites holding state.
    pub fn sites(&self) -> impl Iterator<Item = &str> {
        self.sites.keys().map(String::as_str)
    }

    /// Joins still waiting for a sample, summary or settle.
    #[must_use]
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// Restore a site from disk. The site must hold no state yet. Dictionary
    /// entries are inserted by [`Dict::insert`] rules; those tagged with the
    /// current or previous hour are marked written. `recent` is loaded as
    /// flushed.
    pub fn load_site(
        &mut self,
        site: &str,
        rollups: Vec<MinuteRollup>,
        dict_entries: Vec<(u64, u64, DictEntry)>,
        recent: Vec<FinalRequest>,
        dismissed: BTreeSet<u64>,
        now: u64,
    ) -> Result<(), InsightsError> {
        if self.sites.get(site).is_some_and(|s| !s.is_empty()) {
            return Err(InsightsError::SiteNotEmpty {
                site: site.to_owned(),
            });
        }
        let generation = self.sites.get(site).map_or(0, |s| s.generation);
        let mut st = SiteState::new(now, generation);
        let mut by_minute: BTreeMap<u64, MinuteRollup> = BTreeMap::new();
        for r in rollups {
            by_minute
                .entry(r.minute)
                .or_insert_with(|| MinuteRollup::new(r.minute))
                .merge(&r);
        }
        st.rollups = by_minute.into_values().collect();
        let current = hour_of(now);
        let previous = hour_of(now.saturating_sub(HOUR_MS));
        let mut written: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
        for (hour, key, entry) in dict_entries {
            st.dict.insert(key, entry);
            if hour == current || hour == previous {
                written.entry(hour).or_default().push(key);
            }
        }
        for (hour, keys) in written {
            st.dict.mark_written(hour, keys);
        }
        let mut recent = recent;
        recent.sort_by_key(|r| r.ts_ms);
        st.recent = recent
            .into_iter()
            .map(|req| RecentEntry {
                req,
                detail: None,
                flushed: true,
                taken: false,
            })
            .collect();
        st.open_issues = open_issue_keys(st.rollups.iter());
        st.dismissed = dismissed;
        self.sites.insert(site.to_owned(), st);
        Ok(())
    }
}

/// Filter, sort and limit requests (at most [`LIST_LIMIT_MAX`]). Ties sort by
/// id so merged memory and disk results are stable.
#[must_use]
pub fn select_requests<'a>(
    requests: impl IntoIterator<Item = &'a FinalRequest>,
    query: Option<&str>,
    sort: InsightsSort,
    limit: u32,
) -> Vec<FinalRequest> {
    let mut rows: Vec<&FinalRequest> = requests
        .into_iter()
        .filter(|r| query.map_or(true, |q| r.matches(q)))
        .collect();
    match sort {
        InsightsSort::Slowest => rows.sort_by(|a, b| {
            b.duration_us
                .cmp(&a.duration_us)
                .then_with(|| b.id.cmp(&a.id))
        }),
        _ => rows.sort_by(|a, b| b.ts_ms.cmp(&a.ts_ms).then_with(|| b.id.cmp(&a.id))),
    }
    rows.into_iter()
        .take(limit.min(LIST_LIMIT_MAX) as usize)
        .cloned()
        .collect()
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests;
