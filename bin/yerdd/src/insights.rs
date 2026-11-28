//! Insights wiring: the shared aggregator, store and request tap, the
//! background tasks that feed and flush them, and the IPC handlers.
//!
//! All behaviour lives in `yerd-insights` (aggregation, codec, store) and
//! `yerd-proxy` (the tap). This module only schedules it:
//!
//! * The **startup task** loads the store under `flushing`, then spawns the
//!   drain task and runs the 1 s tick loop itself. On shutdown it runs a final
//!   forced flush, so awaiting its handle waits for that flush.
//! * The **drain task** moves proxy samples from the tap channel into the
//!   aggregator, one lock per `recv_many` batch.
//! * The **tick** takes `flushing`, then briefly the aggregator lock to tick
//!   and take flush batches, writes them on a blocking thread, re-locks to
//!   confirm or fail them, and prunes when an hour boundary passed or enough
//!   bytes were appended.
//!
//! Lock order is `flushing` then `agg`; `agg` is never held across an await
//! of disk work, and the config mutex is never held while either is taken.
//! Mutating handlers (disable flush, dismiss, clear) take `flushing` first,
//! so they wait for the startup load and never land between a batch being
//! taken and confirmed. Read handlers never take `flushing` and never create
//! site state.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::{mpsc, watch, Mutex};
use tokio::task::JoinHandle;

use yerd_insights::pure::aggregator::select_requests;
use yerd_insights::pure::contract::features_param;
use yerd_insights::pure::model::parse_key_hex;
use yerd_insights::pure::retention::{hour_of, PRUNE_CHECK_BYTES};
use yerd_insights::{Aggregator, FlushBatch, Store, Thresholds};
use yerd_ipc::{ErrorCode, InsightsRange, InsightsSort, Response};
use yerd_proxy::{RequestTap, TapMessage};

use crate::state::DaemonState;

/// Proxy samples moved into the aggregator per lock.
const DRAIN_BATCH: usize = 256;

/// How often the tick runs.
const TICK: Duration = Duration::from_secs(1);

/// Live Insights state shared by the proxy, the dump server, the background
/// tasks and the IPC handlers.
pub struct Insights {
    /// The pure aggregator. Never held across an await of disk work.
    pub agg: Mutex<Aggregator>,
    /// The on-disk store, used only from blocking threads.
    pub store: Arc<Store>,
    /// The proxy's request tap; its setters mirror config and the dump server.
    pub tap: Arc<RequestTap>,
    /// Held from a flush batch being taken until it is confirmed or failed,
    /// and for the whole startup load. Mutating handlers take it first.
    pub flushing: Mutex<()>,
    rx: StdMutex<Option<mpsc::Receiver<TapMessage>>>,
    prune_check_bytes: u64,
}

impl Insights {
    /// Build the Insights state over `store`, with a tap whose ids start with
    /// `nonce`, seeded from `config`: the recording sites, the dump-server
    /// port, the capture switch, the resolved features and the thresholds.
    #[must_use]
    pub fn new(store: Store, nonce: u64, config: &yerd_config::Config) -> Self {
        let (tap, rx) = RequestTap::new(yerd_proxy::insights::DEFAULT_CHANNEL_CAPACITY, nonce);
        tap.set_enabled_sites(config.insights.sites.iter().cloned());
        sync_tap(&tap, &config.dumps);
        let mut agg = Aggregator::new();
        agg.set_thresholds(thresholds_from_config(&config.insights.thresholds));
        Self {
            agg: Mutex::new(agg),
            store: Arc::new(store),
            tap,
            flushing: Mutex::new(()),
            rx: StdMutex::new(Some(rx)),
            prune_check_bytes: PRUNE_CHECK_BYTES,
        }
    }

    /// The store root for a daemon's state directory.
    #[must_use]
    pub fn store_root(state_dir: &std::path::Path) -> PathBuf {
        state_dir.join("insights")
    }

    fn take_rx(&self) -> Option<mpsc::Receiver<TapMessage>> {
        self.rx
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }
}

/// Mirror the `[dumps]` section into the tap: port, capture switch and the
/// resolved feature list.
pub(crate) fn sync_tap(tap: &RequestTap, dumps: &yerd_config::DumpsSection) {
    tap.set_port(dumps.port);
    tap.set_capture(dumps.enabled);
    tap.set_features(&features_param(&crate::dump_server::resolved_features(
        dumps,
    )));
}

/// A random boot nonce for request ids.
#[must_use]
pub fn boot_nonce() -> u64 {
    rand::random::<u64>()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Map a proxy sample onto the aggregator's sample, field for field.
#[must_use]
pub fn sample_from_proxy(s: yerd_proxy::ProxySample) -> yerd_insights::ProxySample {
    yerd_insights::ProxySample {
        id: s.id,
        site: s.site,
        php: s.php,
        method: s.method,
        path: s.path,
        script: s.script,
        status: s.status,
        bytes: s.bytes,
        content_type: s.content_type,
        start_us: s.start_us,
        duration_us: s.duration_us,
        aborted: s.aborted,
        proxy_error: s.proxy_error,
        traced: s.traced,
    }
}

/// Persisted thresholds to the aggregator's detection thresholds.
#[must_use]
pub fn thresholds_from_config(t: &yerd_config::InsightsThresholds) -> Thresholds {
    Thresholds {
        n_plus_one_min: t.n_plus_one_min,
        duplicate_min: t.duplicate_min,
        slow_query_ms: t.slow_query_ms,
        large_read_rows: t.large_read_rows,
        max_queries: t.max_queries,
    }
}

/// Persisted thresholds to the wire type.
#[must_use]
pub fn thresholds_to_ipc(t: &yerd_config::InsightsThresholds) -> yerd_ipc::InsightsThresholds {
    yerd_ipc::InsightsThresholds {
        n_plus_one_min: t.n_plus_one_min,
        duplicate_min: t.duplicate_min,
        slow_query_ms: t.slow_query_ms,
        large_read_rows: t.large_read_rows,
        max_queries: t.max_queries,
    }
}

/// Wire thresholds to the persisted type.
#[must_use]
pub fn thresholds_from_ipc(t: &yerd_ipc::InsightsThresholds) -> yerd_config::InsightsThresholds {
    yerd_config::InsightsThresholds {
        n_plus_one_min: t.n_plus_one_min,
        duplicate_min: t.duplicate_min,
        slow_query_ms: t.slow_query_ms,
        large_read_rows: t.large_read_rows,
        max_queries: t.max_queries,
    }
}

// ---------- background tasks ----------

/// Spawn the startup task: load under `flushing`, then the drain task and
/// the tick loop until shutdown, then a final forced flush. A shutdown that
/// arrives during the load skips the loop and flushes straight away.
pub fn spawn(ins: Arc<Insights>, shutdown_rx: watch::Receiver<bool>) -> JoinHandle<()> {
    tokio::spawn(run(ins, shutdown_rx))
}

async fn run(ins: Arc<Insights>, shutdown_rx: watch::Receiver<bool>) {
    let Some(mut rx) = ins.take_rx() else {
        return;
    };
    load(&ins).await;
    let mut tick = TickState::default();
    if *shutdown_rx.borrow() {
        drain_now(&ins, &mut rx).await;
        tick_once(&ins, now_ms(), true, &mut tick).await;
        return;
    }
    let drain = tokio::spawn(drain(ins.clone(), rx, shutdown_rx.clone()));
    let mut interval = tokio::time::interval(TICK);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut stop = shutdown_rx;
    loop {
        tokio::select! {
            _ = interval.tick() => tick_once(&ins, now_ms(), false, &mut tick).await,
            _ = stop.changed() => break,
        }
    }
    let _ = drain.await;
    tick_once(&ins, now_ms(), true, &mut tick).await;
}

/// Load every stored site into the aggregator, holding `flushing`
/// throughout. Never fails: an unreadable store or site loads empty.
pub(crate) async fn load(ins: &Insights) {
    let _flushing = ins.flushing.lock().await;
    let now = now_ms();
    let store = ins.store.clone();
    let sites = match tokio::task::spawn_blocking(move || store.load_all(now)).await {
        Ok(Ok(sites)) => sites,
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "insights store could not be listed; starting empty");
            Vec::new()
        }
        Err(e) => {
            tracing::warn!(error = %e, "insights load task failed; starting empty");
            Vec::new()
        }
    };
    let mut agg = ins.agg.lock().await;
    for l in sites {
        if let Some(e) = &l.error {
            tracing::warn!(site = %l.site, error = %e, "insights site could not be read; starting it empty");
        }
        if let Err(e) = agg.load_site(
            &l.site,
            l.rollups,
            l.dict_entries,
            l.recent,
            l.dismissed,
            now,
        ) {
            tracing::warn!(site = %l.site, error = %e, "insights site load skipped");
        }
    }
}

async fn drain(
    ins: Arc<Insights>,
    mut rx: mpsc::Receiver<TapMessage>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let mut buf = Vec::with_capacity(DRAIN_BATCH);
    loop {
        tokio::select! {
            n = rx.recv_many(&mut buf, DRAIN_BATCH) => {
                if n == 0 {
                    return;
                }
                ingest(&ins, &mut buf).await;
            }
            _ = shutdown_rx.changed() => {
                ingest(&ins, &mut buf).await;
                drain_now(&ins, &mut rx).await;
                return;
            }
        }
    }
}

/// Ingest whatever is already queued on the tap channel, without waiting.
async fn drain_now(ins: &Insights, rx: &mut mpsc::Receiver<TapMessage>) {
    let mut buf = Vec::with_capacity(DRAIN_BATCH);
    while let Ok(m) = rx.try_recv() {
        buf.push(m);
        if buf.len() >= DRAIN_BATCH {
            ingest(ins, &mut buf).await;
        }
    }
    ingest(ins, &mut buf).await;
}

/// Fold a batch of tap messages into the aggregator under one lock.
pub(crate) async fn ingest(ins: &Insights, buf: &mut Vec<TapMessage>) {
    if buf.is_empty() {
        return;
    }
    let now = now_ms();
    let mut agg = ins.agg.lock().await;
    for m in buf.drain(..) {
        match m {
            TapMessage::Sample(s) => agg.on_sample(sample_from_proxy(s), now),
            TapMessage::Static { site, ts_ms } => agg.note_static(&site, ts_ms, now),
        }
    }
}

/// Tick-loop bookkeeping kept between ticks.
#[derive(Debug, Default)]
pub(crate) struct TickState {
    bytes_since_prune: u64,
    last_prune_hour: Option<u64>,
}

/// One tick under `flushing`: fold proxy drops, tick, flush what is due (all
/// of it when `force`), then prune when an hour boundary passed or
/// [`PRUNE_CHECK_BYTES`] were appended since the last prune.
pub(crate) async fn tick_once(ins: &Insights, now: u64, force: bool, st: &mut TickState) {
    let _flushing = ins.flushing.lock().await;
    let (batches, dismissed_changed) = {
        let mut agg = ins.agg.lock().await;
        for (site, n) in ins.tap.take_dropped() {
            agg.note_proxy_dropped(&site, n, now);
        }
        let outcome = agg.tick(now);
        (agg.take_flush(now, force, None), outcome.dismissed_changed)
    };
    st.bytes_since_prune = st
        .bytes_since_prune
        .saturating_add(write_batches(ins, batches, now).await);
    let hour = hour_of(now);
    if st.last_prune_hour != Some(hour) || st.bytes_since_prune >= ins.prune_check_bytes {
        prune(ins, now).await;
        st.bytes_since_prune = 0;
        st.last_prune_hour = Some(hour);
    }
    for site in dismissed_changed {
        save_dismissed(ins, &site).await;
    }
}

/// Write `batches` on blocking threads, then confirm or fail each under the
/// aggregator lock. The caller holds `flushing`. Returns the bytes written.
pub(crate) async fn write_batches(ins: &Insights, batches: Vec<FlushBatch>, now: u64) -> u64 {
    let mut written = 0u64;
    for batch in batches {
        let site = batch.site.clone();
        let generation = batch.generation;
        let ids = batch.ids();
        let hours = batch.hours();
        let store = ins.store.clone();
        let result = tokio::task::spawn_blocking(move || store.append_batch(&batch)).await;
        let mut agg = ins.agg.lock().await;
        match result {
            Ok(Ok(n)) => {
                written = written.saturating_add(n);
                agg.confirm_flush(&site, generation, &ids);
            }
            Ok(Err(e)) => {
                tracing::warn!(site = %site, error = %e, "insights flush failed; batch counted as dropped");
                agg.flush_failed(&site, generation, &ids, &hours, now);
            }
            Err(e) => {
                tracing::warn!(site = %site, error = %e, "insights flush task failed; batch counted as dropped");
                agg.flush_failed(&site, generation, &ids, &hours, now);
            }
        }
    }
    written
}

async fn prune(ins: &Insights, now: u64) {
    let store = ins.store.clone();
    let removed = match tokio::task::spawn_blocking(move || store.prune(now)).await {
        Ok(Ok(removed)) => removed,
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "insights prune failed");
            return;
        }
        Err(e) => {
            tracing::warn!(error = %e, "insights prune task failed");
            return;
        }
    };
    if removed.is_empty() {
        return;
    }
    let mut by_site: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    for (site, hour) in removed {
        by_site.entry(site).or_default().push(hour);
    }
    let mut agg = ins.agg.lock().await;
    for (site, hours) in by_site {
        agg.on_pruned(&site, &hours);
    }
}

async fn save_dismissed(ins: &Insights, site: &str) {
    let keys = ins.agg.lock().await.dismissed_keys(site);
    let store = ins.store.clone();
    let owned = site.to_owned();
    match tokio::task::spawn_blocking(move || store.save_dismissed(&owned, &keys)).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!(site, error = %e, "insights dismissals not saved"),
        Err(e) => tracing::warn!(site, error = %e, "insights dismissals task failed"),
    }
}

/// Force-flush one site now, under `flushing`.
pub(crate) async fn flush_site(ins: &Insights, site: &str) {
    let _flushing = ins.flushing.lock().await;
    let now = now_ms();
    let batches = ins.agg.lock().await.take_flush(now, true, Some(site));
    write_batches(ins, batches, now).await;
}

// ---------- IPC handlers (called from `ipc_server`) ----------

fn error(code: ErrorCode, message: impl Into<String>) -> Response {
    Response::Error {
        code,
        message: message.into(),
    }
}

/// `InsightsStatus`: the recording sites, thresholds, the global capture
/// switch and the open issues across the recording sites.
pub async fn status(state: &DaemonState) -> Response {
    let (sites, thresholds, capture_enabled) = {
        let c = state.config.lock().await;
        (
            c.insights.sites.iter().cloned().collect::<Vec<_>>(),
            thresholds_to_ipc(&c.insights.thresholds),
            c.dumps.enabled,
        )
    };
    let open_issues = {
        let agg = state.insights.agg.lock().await;
        sites
            .iter()
            .map(|s| agg.open_issue_count(s))
            .fold(0u32, u32::saturating_add)
    };
    Response::InsightsStatus {
        sites,
        thresholds,
        capture_enabled,
        open_issues,
    }
}

/// `SetInsightsEnabled`: persist the lowercased site in
/// `config.insights.sites` and mirror the set into the tap. Disabling
/// force-flushes the site; enabling a site on a non-legacy PHP version checks
/// the published extension in the background and restarts only the pools
/// whose `.so` was placed or replaced.
pub async fn set_enabled(state: &Arc<DaemonState>, site: &str, enabled: bool) -> Response {
    let key = site.trim().to_lowercase();
    if key.is_empty() {
        return error(ErrorCode::NotFound, "site name is empty");
    }
    let capture = {
        let mut cfg = state.config.lock().await;
        let before = cfg.insights.sites.clone();
        let changed = if enabled {
            cfg.insights.sites.insert(key.clone())
        } else {
            cfg.insights.sites.remove(&key)
        };
        if changed {
            if let Err(e) = cfg.save(&state.config_path) {
                cfg.insights.sites = before;
                return error(ErrorCode::Internal, format!("failed to save config: {e}"));
            }
        }
        state
            .insights
            .tap
            .set_enabled_sites(cfg.insights.sites.iter().cloned());
        cfg.dumps.enabled
    };
    let routed = state
        .router
        .read()
        .await
        .iter()
        .find(|s| s.name().to_lowercase() == key)
        .map(|s| (s.name().to_owned(), s.php()));
    let name = routed.as_ref().map_or(key, |(n, _)| n.clone());
    if !enabled {
        flush_site(&state.insights, &name).await;
        return Response::Ok;
    }
    if let Some((_, php)) = routed {
        if capture && !php.is_legacy() {
            let state = state.clone();
            tokio::spawn(async move {
                crate::dump_server::refresh_ext_and_restart(&state).await;
            });
        }
    }
    Response::Ok
}

/// `SetInsightsThresholds`: validate, persist, and apply to new requests.
pub async fn set_thresholds(state: &DaemonState, t: &yerd_ipc::InsightsThresholds) -> Response {
    let next = thresholds_from_ipc(t);
    if !next.is_valid() {
        return error(
            ErrorCode::InvalidSetting,
            "thresholds must be at least 1, and the duplicate minimum at least 2",
        );
    }
    {
        let mut cfg = state.config.lock().await;
        let before = cfg.insights.thresholds;
        cfg.insights.thresholds = next;
        if let Err(e) = cfg.save(&state.config_path) {
            cfg.insights.thresholds = before;
            return error(ErrorCode::Internal, format!("failed to save config: {e}"));
        }
        state
            .insights
            .agg
            .lock()
            .await
            .set_thresholds(thresholds_from_config(&next));
    }
    Response::Ok
}

/// `InsightsOverview`: computed under the aggregator lock; `ext_available`
/// is whether the site's PHP version has an extension build (not legacy).
pub async fn overview(state: &DaemonState, site: &str, range: InsightsRange) -> Response {
    let mut overview = state
        .insights
        .agg
        .lock()
        .await
        .overview(site, range, now_ms());
    overview.ext_available = state
        .router
        .read()
        .await
        .get(site)
        .is_some_and(|s| !s.php().is_legacy());
    Response::InsightsOverview {
        overview: Box::new(overview),
    }
}

/// `ListInsightsRequests`: 5m is served from memory alone; 1h/24h merge the
/// in-memory requests with the cached store scan, de-duplicated by id.
pub async fn list_requests(
    state: &DaemonState,
    site: &str,
    range: InsightsRange,
    query: Option<String>,
    sort: InsightsSort,
    limit: u32,
) -> Response {
    let now = now_ms();
    let query = query.filter(|q| !q.trim().is_empty());
    let memory = state.insights.agg.lock().await.recent_requests(
        site,
        range,
        query.as_deref(),
        sort,
        limit,
        now,
    );
    let rows = if range == InsightsRange::FiveMinutes {
        memory
    } else {
        let store = state.insights.store.clone();
        let owned_site = site.to_owned();
        let owned_query = query.clone();
        let disk = tokio::task::spawn_blocking(move || {
            store.scan_requests(&owned_site, range, owned_query.as_deref(), sort, limit, now)
        })
        .await;
        let disk = match disk {
            Ok(Ok(rows)) => rows,
            Ok(Err(e)) => {
                tracing::warn!(site, error = %e, "insights request scan failed; listing memory only");
                Vec::new()
            }
            Err(e) => {
                tracing::warn!(site, error = %e, "insights request scan task failed");
                Vec::new()
            }
        };
        let seen: HashSet<&str> = memory.iter().map(|r| r.id.as_str()).collect();
        let merged = memory
            .iter()
            .chain(disk.iter().filter(|r| !seen.contains(r.id.as_str())));
        select_requests(merged, None, sort, limit)
    };
    Response::InsightsRequests {
        requests: rows
            .iter()
            .map(yerd_insights::FinalRequest::to_summary)
            .collect(),
    }
}

/// `GetInsightsRequest`: memory first, then the store's hour file for
/// `ts_ms`. An unknown id is `NotFound`.
pub async fn get_request(state: &DaemonState, site: &str, id: &str, ts_ms: u64) -> Response {
    if let Some(d) = state.insights.agg.lock().await.recent_detail(site, id) {
        return Response::InsightsRequest {
            request: Box::new(d),
        };
    }
    let store = state.insights.store.clone();
    let (owned_site, owned_id) = (site.to_owned(), id.to_owned());
    match tokio::task::spawn_blocking(move || store.find_detail(&owned_site, &owned_id, ts_ms))
        .await
    {
        Ok(Ok(Some((req, detail)))) => Response::InsightsRequest {
            request: Box::new(detail.to_ipc(&req)),
        },
        Ok(Ok(None)) => error(ErrorCode::NotFound, format!("no recorded request {id}")),
        Ok(Err(e)) => error(ErrorCode::Internal, format!("insights read failed: {e}")),
        Err(e) => error(
            ErrorCode::Internal,
            format!("insights read task failed: {e}"),
        ),
    }
}

/// `DismissInsightsIssue`: update the aggregator under `flushing`, then
/// persist the site's dismissed keys still present in its dictionary.
pub async fn dismiss(state: &DaemonState, site: &str, key: &str, dismissed: bool) -> Response {
    let Some(key) = parse_key_hex(key) else {
        return error(
            ErrorCode::InvalidSetting,
            format!("invalid issue key `{key}`"),
        );
    };
    let _flushing = state.insights.flushing.lock().await;
    let changed = state
        .insights
        .agg
        .lock()
        .await
        .dismiss(site, key, dismissed);
    if changed {
        save_dismissed(&state.insights, site).await;
    }
    Response::Ok
}

/// `ClearInsights`: under `flushing`, reset the site in the aggregator, then
/// delete its files (which also invalidates the store's scan cache).
pub async fn clear(state: &DaemonState, site: &str) -> Response {
    let _flushing = state.insights.flushing.lock().await;
    state.insights.agg.lock().await.clear(site, now_ms());
    let store = state.insights.store.clone();
    let owned = site.to_owned();
    match tokio::task::spawn_blocking(move || store.clear_site(&owned)).await {
        Ok(Ok(())) => Response::Ok,
        Ok(Err(e)) => error(ErrorCode::Internal, format!("insights clear failed: {e}")),
        Err(e) => error(
            ErrorCode::Internal,
            format!("insights clear task failed: {e}"),
        ),
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests;
