//! Loopback dump server: receives telemetry frames from the `yerd-php-ext`
//! extension and buffers them for the GUI.
//!
//! The extension opens a TCP connection to `127.0.0.1:<port>` and writes
//! newline-delimited JSON frames (`{category, ts, site, request_id, payload}`).
//! This module parses each line into a [`DumpEvent`], assigns it a monotonic id,
//! and stores it in a bounded ring on [`DumpStore`]. The IPC layer pages the ring
//! to the GUI via `ListDumps`.
//!
//! Frames are untrusted (anything on loopback could connect): malformed lines are
//! dropped, lines are size-capped, and nothing here ever panics.

use std::collections::VecDeque;
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{watch, Mutex, Notify};

use yerd_config::DumpsSection;
use yerd_ipc::{DumpCategory, DumpCounts, DumpEvent, DumpExtStatus, Response};
use yerd_platform::PlatformDirs;

use crate::insights::Insights;
use crate::state::DaemonState;

/// Maximum number of events retained in the ring before the oldest are evicted.
const RING_CAP: usize = 2000;
/// How many recently-removed ids to remember, so an incrementally-polling client
/// can reconcile deletes/evictions it still holds. A bounded best-effort log:
/// the `min_live_id` cursor (see [`DumpRing::list`]) is the real correctness
/// guarantee for clients dropping evicted rows.
const REMOVED_LOG_CAP: usize = 4096;
/// In non-persist mode, how many distinct recent requests to retain. A rolling
/// window (rather than "only the latest request") so concurrent requests -
/// whose frames interleave at the ring from separate FPM workers - coexist
/// instead of clearing each other on every cross-request frame.
const RETAINED_REQUESTS: usize = 25;
/// Hard cap on a single newline-delimited frame, including its `\n`. It
/// matches the extension's `MAX_LINE`, so every frame the extension sends
/// fits. The cap applies per line after splitting, so a near-limit line
/// followed by the next frame in the same read is still accepted; an
/// unterminated line that reaches the cap drops the connection.
const MAX_LINE_BYTES: usize = 256 * 1024;

/// The telemetry feature keys mirrored into the extension's state file.
const FEATURES: &[&str] = &[
    "dumps", "queries", "jobs", "views", "requests", "logs", "cache", "http",
];

/// Default capture state for a feature when the user hasn't set it. Most default
/// on; outgoing-`http` capture is opt-in (extra overhead, less commonly needed,
/// and only emitted by extension v0.1.4+).
fn feature_default(name: &str) -> bool {
    name != "http"
}

/// Every known feature with its effective value: the configured one, else
/// [`feature_default`].
pub(crate) fn resolved_features(dumps: &DumpsSection) -> std::collections::BTreeMap<String, bool> {
    FEATURES
        .iter()
        .map(|&f| {
            (
                f.to_owned(),
                dumps
                    .features
                    .get(f)
                    .copied()
                    .unwrap_or_else(|| feature_default(f)),
            )
        })
        .collect()
}

/// Shared dump-telemetry state: the ring plus a rebind signal and a bound flag.
pub struct DumpStore {
    ring: Mutex<DumpRing>,
    /// Notified when the configured port changes so the server rebinds.
    rebind: Notify,
    /// Whether the server is currently bound and accepting.
    bound: AtomicBool,
    /// Whether logs persist across requests. `false` (default) clears the
    /// non-pinned buffer on each new request. Mirrors the config; read on every
    /// incoming frame.
    persist: AtomicBool,
}

impl DumpStore {
    /// A fresh, empty store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            ring: Mutex::new(DumpRing::new()),
            rebind: Notify::new(),
            bound: AtomicBool::new(false),
            persist: AtomicBool::new(false),
        }
    }

    /// Signal the server task to rebind (after a port change).
    pub fn request_rebind(&self) {
        self.rebind.notify_one();
    }

    /// Update the persist flag (mirrors config).
    pub fn set_persist(&self, persist: bool) {
        self.persist.store(persist, Ordering::Release);
    }
}

impl Default for DumpStore {
    fn default() -> Self {
        Self::new()
    }
}

/// A page of the ring for one `ListDumps` poll.
struct DumpPage {
    events: Vec<DumpEvent>,
    removed: Vec<u64>,
    counts: DumpCounts,
    latest_id: u64,
    /// Smallest id still buffered (or `next_id + 1` when empty). Clients drop any
    /// held id below this unconditionally, so dropping evicted/cleared rows never
    /// depends on the bounded `removed` log.
    min_live_id: u64,
}

/// Bounded in-memory buffer of [`DumpEvent`]s.
struct DumpRing {
    events: VecDeque<DumpEvent>,
    removed: VecDeque<u64>,
    next_id: u64,
    /// Distinct request_ids retained, oldest first. In non-persist mode the
    /// window is capped at [`RETAINED_REQUESTS`]; evicting a request drops its
    /// events. `events` always stays ascending by id (push_back / pop_front /
    /// order-preserving removals), so `events.front()` is the smallest live id.
    recent_requests: VecDeque<String>,
}

impl DumpRing {
    const fn new() -> Self {
        Self {
            events: VecDeque::new(),
            removed: VecDeque::new(),
            next_id: 0,
            recent_requests: VecDeque::new(),
        }
    }

    fn push(&mut self, frame: IncomingFrame, now_ms: u64, persist: bool) {
        if !self.recent_requests.iter().any(|r| r == &frame.request_id) {
            self.recent_requests.push_back(frame.request_id.clone());
            while self.recent_requests.len() > RETAINED_REQUESTS {
                if let Some(old) = self.recent_requests.pop_front() {
                    if !persist {
                        self.evict_request(&old);
                    }
                }
            }
        }
        self.next_id = self.next_id.saturating_add(1);
        let ts_ms = if frame.ts != 0 { frame.ts } else { now_ms };
        self.events.push_back(DumpEvent {
            id: self.next_id,
            category: frame.category,
            ts_ms,
            site: frame.site,
            request_id: frame.request_id,
            payload: frame.payload,
        });
        while self.events.len() > RING_CAP {
            if let Some(ev) = self.events.pop_front() {
                self.note_removed(ev.id);
            }
        }
    }

    fn note_removed(&mut self, id: u64) {
        self.removed.push_back(id);
        self.trim_removed();
    }

    fn trim_removed(&mut self) {
        while self.removed.len() > REMOVED_LOG_CAP {
            self.removed.pop_front();
        }
    }

    /// Drop all events belonging to `req` (order-preserving), noting each id.
    fn evict_request(&mut self, req: &str) {
        let mut kept = VecDeque::with_capacity(self.events.len());
        for ev in self.events.drain(..) {
            if ev.request_id == req {
                self.removed.push_back(ev.id);
            } else {
                kept.push_back(ev);
            }
        }
        self.events = kept;
        self.trim_removed();
    }

    fn delete(&mut self, id: u64) {
        if let Some(idx) = self.events.iter().position(|e| e.id == id) {
            if self.events.remove(idx).is_some() {
                self.note_removed(id);
            }
        }
    }

    fn clear(&mut self) {
        for ev in self.events.drain(..) {
            self.removed.push_back(ev.id);
        }
        self.recent_requests.clear();
        self.trim_removed();
    }

    fn counts(&self) -> DumpCounts {
        let mut c = DumpCounts::default();
        for e in &self.events {
            c.increment(e.category);
        }
        c
    }

    /// Page of events newer than `since_id`, plus the removed ids the client may
    /// hold, the counts, the next cursor (`latest_id`), and `min_live_id`.
    fn list(&self, since_id: u64) -> DumpPage {
        let events = self
            .events
            .iter()
            .filter(|e| e.id > since_id)
            .cloned()
            .collect();
        let removed = self
            .removed
            .iter()
            .copied()
            .filter(|&id| id <= since_id)
            .collect();
        let min_live_id = self
            .events
            .front()
            .map_or(self.next_id.saturating_add(1), |e| e.id);
        DumpPage {
            events,
            removed,
            counts: self.counts(),
            latest_id: self.next_id,
            min_live_id,
        }
    }
}

/// A frame as sent by the extension. `ts` is epoch milliseconds; 0 (or absent)
/// means "stamp on receipt". `v`, `seq` and `t_us` are the protocol-2 envelope
/// additions (contract C3); a frame with `v >= 2` also feeds Insights.
#[derive(Deserialize)]
struct IncomingFrame {
    category: DumpCategory,
    #[serde(default)]
    ts: u64,
    #[serde(default)]
    site: String,
    #[serde(default)]
    request_id: String,
    payload: serde_json::Value,
    #[serde(default)]
    v: u8,
    #[serde(default)]
    seq: Option<u32>,
    #[serde(default)]
    t_us: Option<u64>,
}

impl IncomingFrame {
    /// The Insights view of a protocol-2 frame, `None` for a legacy frame.
    fn insights_frame(&self) -> Option<yerd_insights::ExtFrame> {
        (self.v >= yerd_insights::pure::contract::PROTOCOL_VERSION).then(|| {
            yerd_insights::ExtFrame {
                request_id: self.request_id.clone(),
                site: self.site.clone(),
                category: self.category,
                seq: self.seq,
                t_us: self.t_us,
                payload: self.payload.clone(),
            }
        })
    }
}

/// Record whether the dump server is bound, on the store and the Insights tap.
fn set_bound(state: &DaemonState, bound: bool) {
    state.dumps.bound.store(bound, Ordering::Release);
    state.insights.tap.set_bound(bound);
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Serve the dump server until `shutdown_rx` fires, rebinding when the configured
/// port changes. A bind failure (port in use) is logged and retried on the next
/// rebind signal rather than crashing the daemon.
pub async fn run(state: Arc<DaemonState>, mut shutdown_rx: watch::Receiver<bool>) {
    loop {
        let port = state.config.lock().await.dumps.port;
        let listener = match TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await {
            Ok(l) => {
                set_bound(&state, true);
                tracing::info!(port, "dump server bound");
                l
            }
            Err(e) => {
                set_bound(&state, false);
                tracing::warn!(port, error = %e, "dump server bind failed; will retry on port change");
                tokio::select! {
                    () = state.dumps.rebind.notified() => continue,
                    _ = shutdown_rx.changed() => return,
                }
            }
        };

        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    set_bound(&state, false);
                    return;
                }
                () = state.dumps.rebind.notified() => break,
                accepted = listener.accept() => {
                    match accepted {
                        Ok((stream, _peer)) => {
                            let store = state.dumps.clone();
                            let insights = state.insights.clone();
                            tokio::spawn(handle_conn(stream, store, insights));
                        }
                        Err(e) => tracing::debug!(error = %e, "dump server accept failed"),
                    }
                }
            }
        }
        set_bound(&state, false);
    }
}

/// Read newline-delimited JSON frames from one connection until EOF, pushing each
/// valid frame into the ring and each protocol-2 frame into the Insights
/// aggregator. Bounded: complete lines at or over [`MAX_LINE_BYTES`] and
/// malformed lines are skipped; an unterminated line that reaches the cap drops
/// the connection.
async fn handle_conn(stream: TcpStream, store: Arc<DumpStore>, insights: Arc<Insights>) {
    let mut stream = stream;
    let mut pending: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let n = match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        let Some(slice) = chunk.get(..n) else {
            return;
        };
        pending.extend_from_slice(slice);
        while let Some(pos) = pending.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = pending.drain(..pos).collect();
            pending.drain(..1);
            if line.is_empty() || line.len() >= MAX_LINE_BYTES {
                continue;
            }
            if let Ok(frame) = serde_json::from_slice::<IncomingFrame>(&line) {
                ingest_frame(&store, &insights, frame).await;
            }
        }
        if pending.len() >= MAX_LINE_BYTES {
            return;
        }
    }
}

/// Push one parsed frame into the live ring and, for protocol 2, the Insights
/// aggregator. A protocol-2 `request` summary stays out of the ring while the
/// `requests` feature is off, since trace mode always sends it.
async fn ingest_frame(store: &DumpStore, insights: &Insights, frame: IncomingFrame) {
    let ext = frame.insights_frame();
    let hide = ext.is_some()
        && frame.category == DumpCategory::Request
        && !insights.tap.features().split(',').any(|f| f == "requests");
    if !hide {
        let persist = store.persist.load(Ordering::Acquire);
        store.ring.lock().await.push(frame, now_ms(), persist);
    }
    if let Some(ext) = ext {
        insights.agg.lock().await.on_frame(ext, now_ms());
    }
}

// ---------- IPC handlers (called from `ipc_server::dispatch`) ----------

/// Page the ring for `ListDumps`.
pub async fn list(state: &DaemonState, since_id: u64) -> Response {
    let page = state.dumps.ring.lock().await.list(since_id);
    Response::Dumps {
        events: page.events,
        removed_ids: page.removed,
        counts: page.counts,
        latest_id: page.latest_id,
        min_live_id: page.min_live_id,
    }
}

/// Drop every buffered event.
pub async fn clear(state: &DaemonState) -> Response {
    state.dumps.ring.lock().await.clear();
    Response::Ok
}

/// Delete one event by id.
pub async fn delete(state: &DaemonState, id: u64) -> Response {
    state.dumps.ring.lock().await.delete(id);
    Response::Ok
}

/// Report dump-server status.
pub async fn status(state: &DaemonState) -> Response {
    let (enabled, port, persist, features) = {
        let c = state.config.lock().await;
        let features = FEATURES
            .iter()
            .map(|&f| {
                (
                    (*f).to_string(),
                    c.dumps
                        .features
                        .get(f)
                        .copied()
                        .unwrap_or_else(|| feature_default(f)),
                )
            })
            .collect();
        (c.dumps.enabled, c.dumps.port, c.dumps.persist, features)
    };
    let counts = state.dumps.ring.lock().await.counts();
    let running = state.dumps.bound.load(Ordering::Acquire);
    let extensions = extension_presence(&state.dirs);
    Response::DumpsStatus {
        enabled,
        port,
        running,
        persist,
        extensions,
        counts,
        features,
    }
}

/// Toggle log persistence: persist config, mirror to the runtime flag the dump
/// server reads on each frame.
pub async fn set_persist(state: &DaemonState, persist: bool) -> Response {
    let resp = apply_config(state, |d| d.persist = persist).await;
    if matches!(resp, Response::Ok) {
        state.dumps.set_persist(persist);
    }
    resp
}

/// Set the global capture flag: persist config, mirror it to the Insights tap
/// and rewrite the state file.
///
/// On the first enable, also fetch the extension `.so` for each installed PHP
/// version and restart any started pools so they load it (subsequent toggles
/// never restart FPM). With capture off the proxy stops sending trace params,
/// so the protocol-2 extension records nothing.
pub async fn set_enabled(state: &DaemonState, enabled: bool) -> Response {
    let was = state.config.lock().await.dumps.enabled;
    let resp = apply_config(state, |d| d.enabled = enabled).await;
    if matches!(resp, Response::Ok) && enabled && !was {
        ensure_ext_and_restart(state).await;
    }
    resp
}

/// Download the extension for installed versions (best-effort, time-bounded) and
/// restart the started pools whose `.so` was placed or replaced, so they load
/// it. Pools of versions left unchanged (already current, unpublished or a
/// failed download) keep running. After a timeout the placed set is unknown,
/// so every started pool that now has a `.so` restarts.
pub(crate) async fn ensure_ext_and_restart(state: &DaemonState) {
    let replaced = match fetch_ext(state).await {
        Some(replaced) => replaced,
        None => crate::ext_install::installed_versions(&state.dirs)
            .into_iter()
            .filter(|v| crate::ext_install::so_path(&state.dirs, *v).is_file())
            .collect(),
    };
    restart_started_pools(state, &replaced).await;
}

/// Check the published extension against the installed `.so` files and
/// restart only the started pools whose file was replaced. Unlike
/// [`ensure_ext_and_restart`], a timed-out check restarts nothing: this runs
/// on every Insights enable, where pools usually already have a working `.so`.
pub(crate) async fn refresh_ext_and_restart(state: &DaemonState) {
    if let Some(replaced) = fetch_ext(state).await {
        restart_started_pools(state, &replaced).await;
    }
}

async fn fetch_ext(state: &DaemonState) -> Option<Vec<yerd_core::PhpVersion>> {
    let dl = crate::network::downloader(state).await;
    let ensure = crate::ext_install::ensure_for_installed(&state.dirs, &dl);
    let fetched = tokio::time::timeout(std::time::Duration::from_secs(30), ensure)
        .await
        .ok();
    if fetched.is_none() {
        tracing::warn!("yerd-dump extension download timed out");
    }
    fetched
}

/// Restart the started pools of `versions` so a freshly replaced
/// `yerd-dump.so` is loaded. Versions without a started pool are skipped.
pub(crate) async fn restart_started_pools(state: &DaemonState, versions: &[yerd_core::PhpVersion]) {
    if versions.is_empty() {
        return;
    }
    let mut mgr = state.php_manager.lock().await;
    let started: Vec<_> = mgr
        .snapshots()
        .into_iter()
        .map(|snap| snap.version)
        .filter(|v| versions.contains(v))
        .collect();
    for v in started {
        if let Err(e) = mgr.restart(v).await {
            tracing::warn!(version = %v, error = %e, "failed to restart FPM pool after updating the dump extension");
        }
    }
}

/// Set a per-feature capture flag.
pub async fn set_feature(state: &DaemonState, feature: String, enabled: bool) -> Response {
    if !FEATURES.contains(&feature.as_str()) {
        return Response::Error {
            code: yerd_ipc::ErrorCode::NotFound,
            message: format!("unknown dump feature: {feature}"),
        };
    }
    apply_config(state, |d| {
        d.features.insert(feature, enabled);
    })
    .await
}

/// Set the dump-server port: persist config, rewrite the state file, and trigger
/// a rebind so the server moves to the new port without a daemon restart.
pub async fn set_port(state: &DaemonState, port: u16) -> Response {
    if port == 0 {
        return Response::Error {
            code: yerd_ipc::ErrorCode::Internal,
            message: "dump server port must be non-zero".into(),
        };
    }
    if port == state.config.lock().await.dumps.port {
        return Response::Ok;
    }
    match TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await {
        Ok(l) => drop(l),
        Err(e) => {
            return Response::Error {
                code: yerd_ipc::ErrorCode::PortInUse,
                message: format!("cannot bind dump server port {port}: {e}"),
            };
        }
    }
    let resp = apply_config(state, |d| d.port = port).await;
    if matches!(resp, Response::Ok) {
        state.dumps.request_rebind();
    }
    resp
}

/// Apply a mutation to the `[dumps]` config section, persist it, and rewrite the
/// extension's state file.
async fn apply_config<F: FnOnce(&mut DumpsSection)>(state: &DaemonState, mutate: F) -> Response {
    let snapshot = {
        let mut cfg = state.config.lock().await;
        mutate(&mut cfg.dumps);
        if let Err(e) = cfg.save(&state.config_path) {
            return Response::Error {
                code: yerd_ipc::ErrorCode::Internal,
                message: format!("failed to save config: {e}"),
            };
        }
        cfg.dumps.clone()
    };
    crate::insights::sync_tap(&state.insights.tap, &snapshot);
    if let Err(e) = write_state_file(&state.dirs, &snapshot) {
        tracing::warn!(error = %e, "failed to write dump state file");
    }
    Response::Ok
}

/// The legacy state file older extensions read each request (contract C5).
#[derive(Serialize)]
struct StateFile<'a> {
    enabled: bool,
    port: u16,
    features: std::collections::BTreeMap<&'a str, bool>,
}

/// Write `{state}/dumps/state.json` atomically (temp-then-rename). The features
/// map is fully resolved (every known key present) so the extension needs no
/// default logic.
///
/// `enabled` is always `false` (contracts C1 and C5): a protocol-2 extension
/// never reads this file and arms per request from the proxy's trace params,
/// while an older extension that still reads it must stay inert everywhere so
/// it never captures every site. The byte shape is unchanged.
pub fn write_state_file(dirs: &PlatformDirs, dumps: &DumpsSection) -> std::io::Result<()> {
    let dir = dirs.state.join("dumps");
    std::fs::create_dir_all(&dir)?;
    let features = FEATURES
        .iter()
        .map(|&f| {
            (
                f,
                dumps
                    .features
                    .get(f)
                    .copied()
                    .unwrap_or_else(|| feature_default(f)),
            )
        })
        .collect();
    let body = StateFile {
        enabled: false,
        port: dumps.port,
        features,
    };
    let json = serde_json::to_vec(&body).map_err(std::io::Error::other)?;
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = dir.join(format!("state.json.{}.{}.tmp", std::process::id(), seq));
    let final_path = dir.join("state.json");
    std::fs::write(&tmp, &json)?;
    std::fs::rename(&tmp, &final_path)?;
    Ok(())
}

/// Monotonic counter for unique temp filenames (combined with the pid).
static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Per-installed-version extension presence: for each installed PHP version,
/// whether a matching `.so` exists in `{data}/php-ext/php-<ver>/yerd-dump.so`.
fn extension_presence(dirs: &PlatformDirs) -> Vec<DumpExtStatus> {
    let mut out: Vec<DumpExtStatus> = crate::ext_install::installed_versions(dirs)
        .into_iter()
        .map(|version| DumpExtStatus {
            version,
            present: crate::ext_install::so_path(dirs, version).is_file(),
            legacy: version.is_legacy(),
        })
        .collect();
    out.sort_by_key(|s| (s.version.major, s.version.minor));
    out
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

    fn frame(category: DumpCategory) -> IncomingFrame {
        frame_req(category, "req")
    }

    fn frame_req(category: DumpCategory, req: &str) -> IncomingFrame {
        IncomingFrame {
            category,
            ts: 0,
            site: "blog.test".into(),
            request_id: req.to_string(),
            payload: serde_json::json!({"x": 1}),
            v: 0,
            seq: None,
            t_us: None,
        }
    }

    #[test]
    fn concurrent_requests_do_not_clear_each_other() {
        let mut ring = DumpRing::new();
        ring.push(frame_req(DumpCategory::Query, "a"), 100, false); // id 1
        ring.push(frame_req(DumpCategory::Query, "b"), 100, false); // id 2
        ring.push(frame_req(DumpCategory::Query, "a"), 100, false); // id 3
        ring.push(frame_req(DumpCategory::Dump, "b"), 100, false); // id 4
        let page = ring.list(0);
        assert_eq!(
            page.events.len(),
            4,
            "interleaved concurrent requests coexist, not annihilate"
        );
    }

    #[test]
    fn non_persist_evicts_oldest_request_beyond_window() {
        let mut ring = DumpRing::new();
        for i in 0..RETAINED_REQUESTS + 2 {
            ring.push(frame_req(DumpCategory::Query, &format!("r{i}")), 100, false);
        }
        let page = ring.list(0);
        assert_eq!(page.events.len(), RETAINED_REQUESTS, "window-bounded");
        assert!(!page.events.iter().any(|e| e.id == 1 || e.id == 2));
        assert_eq!(page.min_live_id, 3);
    }

    #[test]
    fn persist_keeps_events_across_requests() {
        let mut ring = DumpRing::new();
        ring.push(frame_req(DumpCategory::Query, "r1"), 100, true); // id 1
        ring.push(frame_req(DumpCategory::Query, "r2"), 100, true); // id 2
        assert_eq!(
            ring.list(0).events.len(),
            2,
            "persist accumulates across requests"
        );
    }

    #[test]
    fn push_assigns_monotonic_ids_and_counts() {
        let mut ring = DumpRing::new();
        ring.push(frame(DumpCategory::Query), 100, false);
        ring.push(frame(DumpCategory::Query), 100, false);
        ring.push(frame(DumpCategory::Dump), 100, false);
        let page = ring.list(0);
        assert_eq!(page.events.len(), 3);
        assert!(page.removed.is_empty());
        assert_eq!(page.counts.queries, 2);
        assert_eq!(page.counts.dumps, 1);
        assert_eq!(page.latest_id, 3);
        assert_eq!(page.min_live_id, 1);
        assert_eq!(page.events.first().map(|e| e.id), Some(1));
    }

    #[test]
    fn list_since_filters_and_reports_removed() {
        let mut ring = DumpRing::new();
        ring.push(frame(DumpCategory::Query), 100, false);
        ring.push(frame(DumpCategory::Query), 100, false); // id 2
        ring.delete(1);
        let page = ring.list(2);
        assert!(page.events.is_empty(), "no events newer than id 2");
        assert_eq!(page.removed, vec![1], "id 1 was deleted and is <= since_id");
        assert_eq!(page.latest_id, 2);
    }

    #[test]
    fn eviction_drops_oldest_over_cap() {
        let mut ring = DumpRing::new();
        for _ in 0..RING_CAP + 10 {
            ring.push(frame(DumpCategory::Query), 100, false);
        }
        let page = ring.list(0);
        assert!(page.events.len() <= RING_CAP);
        let newest = RING_CAP as u64 + 10;
        assert!(
            page.events.iter().any(|e| e.id == newest),
            "newest retained"
        );
        assert!(!page.events.iter().any(|e| e.id == 1), "oldest evicted");
        assert!(page.min_live_id > 1);
    }

    #[test]
    fn clear_drops_everything() {
        let mut ring = DumpRing::new();
        ring.push(frame(DumpCategory::Dump), 100, false); // id 1
        ring.clear();
        let page = ring.list(1);
        assert!(page.events.is_empty());
        assert_eq!(page.removed, vec![1]);
        assert_eq!(page.counts.dumps, 0);
        assert_eq!(page.min_live_id, 2, "min_live_id = next_id + 1 when empty");
    }

    #[test]
    fn incoming_frame_accepts_canonical_wire_shape() {
        // The cross-repo contract with yerd-php-ext (architecture.md §2.2).
        let f: IncomingFrame = serde_json::from_str(
            r#"{"category":"query","ts":1718360452123,"site":"blog.test","request_id":"abc","payload":{"sql":"select 1"}}"#,
        )
        .unwrap();
        assert_eq!(f.category, DumpCategory::Query);
        assert_eq!(f.ts, 1_718_360_452_123);
        assert_eq!(f.site, "blog.test");
        assert_eq!(f.request_id, "abc");
        assert_eq!(f.payload, serde_json::json!({"sql": "select 1"}));
        let minimal: IncomingFrame =
            serde_json::from_str(r#"{"category":"dump","payload":{}}"#).unwrap();
        assert_eq!(minimal.ts, 0);
        assert!(minimal.site.is_empty());
        assert!(minimal.request_id.is_empty());
    }

    #[test]
    fn state_file_byte_shape() {
        // The contract the extension reads each request (architecture.md §2.3).
        let body = StateFile {
            enabled: true,
            port: 2304,
            features: [("queries", true), ("http", false)].into_iter().collect(),
        };
        let s = serde_json::to_string(&body).unwrap();
        assert_eq!(
            s,
            r#"{"enabled":true,"port":2304,"features":{"http":false,"queries":true}}"#
        );
    }

    #[test]
    fn feature_default_only_http_is_off() {
        assert!(!feature_default("http"));
        for f in [
            "dumps", "queries", "jobs", "views", "requests", "logs", "cache",
        ] {
            assert!(feature_default(f), "{f} should default on");
        }
        for &f in FEATURES {
            let _ = feature_default(f);
        }
    }

    #[test]
    fn now_ms_is_nonzero() {
        assert!(now_ms() > 0, "wall clock should be well past the epoch");
    }

    #[test]
    fn push_stamps_now_when_ts_zero_and_preserves_explicit_ts() {
        let mut ring = DumpRing::new();
        ring.push(frame_req(DumpCategory::Dump, "r"), 12_345, false);
        let mut f = frame_req(DumpCategory::Dump, "r");
        f.ts = 999;
        ring.push(f, 12_345, false);
        let page = ring.list(0);
        assert_eq!(page.events[0].ts_ms, 12_345, "stamped on receipt");
        assert_eq!(page.events[1].ts_ms, 999, "explicit ts preserved");
    }

    #[test]
    fn delete_missing_id_is_noop() {
        let mut ring = DumpRing::new();
        ring.push(frame(DumpCategory::Query), 100, false);
        ring.delete(42);
        let page = ring.list(0);
        assert_eq!(page.events.len(), 1);
        assert!(page.removed.is_empty(), "no spurious removed entry");
    }

    #[test]
    fn counts_reflect_only_live_events_after_delete() {
        let mut ring = DumpRing::new();
        ring.push(frame(DumpCategory::Query), 100, false);
        ring.push(frame(DumpCategory::Dump), 100, false);
        ring.delete(2);
        let c = ring.counts();
        assert_eq!(c.queries, 1);
        assert_eq!(c.dumps, 0);
    }

    #[test]
    fn extension_presence_empty_without_installed_php() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = yerd_platform::PlatformDirs {
            config: tmp.path().join("c"),
            data: tmp.path().join("d"),
            state: tmp.path().join("s"),
            cache: tmp.path().join("ca"),
            runtime: tmp.path().join("r"),
        };
        assert!(extension_presence(&dirs).is_empty());
    }

    #[test]
    fn write_state_file_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = yerd_platform::PlatformDirs {
            config: tmp.path().join("c"),
            data: tmp.path().join("d"),
            state: tmp.path().join("s"),
            cache: tmp.path().join("ca"),
            runtime: tmp.path().join("r"),
        };
        let mut features = std::collections::BTreeMap::new();
        features.insert("queries".to_string(), false);
        let dumps = DumpsSection {
            enabled: true,
            features,
            ..DumpsSection::default()
        };
        write_state_file(&dirs, &dumps).unwrap();
        let body = std::fs::read_to_string(dirs.state.join("dumps").join("state.json")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["enabled"], serde_json::json!(false));
        assert_eq!(v["port"], serde_json::json!(2304));
        assert_eq!(v["features"]["queries"], serde_json::json!(false));
        assert_eq!(v["features"]["dumps"], serde_json::json!(true));
    }

    // ---------- `&DaemonState` IPC-handler coverage ----------

    use crate::state::DaemonState;
    use crate::test_support::state_in;

    /// Push a frame directly into the live ring (bypassing the TCP path).
    async fn push_live(state: &DaemonState, category: DumpCategory, req: &str) {
        state
            .dumps
            .ring
            .lock()
            .await
            .push(frame_req(category, req), 100, false);
    }

    #[tokio::test]
    async fn list_handler_pages_live_ring() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        push_live(&state, DumpCategory::Query, "r1").await;
        push_live(&state, DumpCategory::Dump, "r1").await;

        match list(&state, 0).await {
            Response::Dumps {
                events,
                removed_ids,
                counts,
                latest_id,
                min_live_id,
            } => {
                assert_eq!(events.len(), 2);
                assert!(removed_ids.is_empty());
                assert_eq!(counts.queries, 1);
                assert_eq!(counts.dumps, 1);
                assert_eq!(latest_id, 2);
                assert_eq!(min_live_id, 1);
            }
            other => panic!("expected Dumps, got {other:?}"),
        }

        match list(&state, 2).await {
            Response::Dumps { events, .. } => assert!(events.is_empty()),
            other => panic!("expected Dumps, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn delete_handler_removes_one_and_clear_empties() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        push_live(&state, DumpCategory::Query, "r").await;
        push_live(&state, DumpCategory::Query, "r").await;

        assert!(matches!(delete(&state, 1).await, Response::Ok));
        match list(&state, 0).await {
            Response::Dumps { events, .. } => {
                assert_eq!(events.len(), 1);
                assert_eq!(events[0].id, 2);
            }
            other => panic!("expected Dumps, got {other:?}"),
        }

        assert!(matches!(clear(&state).await, Response::Ok));
        match list(&state, 0).await {
            Response::Dumps { events, counts, .. } => {
                assert!(events.is_empty());
                assert_eq!(counts.queries, 0);
            }
            other => panic!("expected Dumps, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn status_handler_reports_defaults_and_counts() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        push_live(&state, DumpCategory::Query, "r").await;

        match status(&state).await {
            Response::DumpsStatus {
                enabled,
                port,
                running,
                persist,
                extensions,
                counts,
                features,
            } => {
                assert!(enabled);
                assert_eq!(port, 2304);
                assert!(!running);
                assert!(!persist);
                assert!(extensions.is_empty());
                assert_eq!(counts.queries, 1);
                assert_eq!(features.len(), FEATURES.len());
                assert_eq!(features.get("http"), Some(&false));
                assert_eq!(features.get("dumps"), Some(&true));
            }
            other => panic!("expected DumpsStatus, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn set_persist_updates_config_flag_and_state_file() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());

        assert!(matches!(set_persist(&state, true).await, Response::Ok));
        assert!(state.config.lock().await.dumps.persist);
        assert!(state.dumps.persist.load(Ordering::Acquire));
        let reloaded = yerd_config::Config::load(&state.config_path).unwrap();
        assert!(reloaded.dumps.persist);
        let sf = state.dirs.state.join("dumps").join("state.json");
        assert!(sf.is_file());

        assert!(matches!(set_persist(&state, false).await, Response::Ok));
        assert!(!state.dumps.persist.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn set_feature_known_persists_unknown_is_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());

        assert!(matches!(
            set_feature(&state, "queries".into(), false).await,
            Response::Ok
        ));
        assert_eq!(
            state.config.lock().await.dumps.features.get("queries"),
            Some(&false)
        );

        match set_feature(&state, "bogus".into(), true).await {
            Response::Error { code, message } => {
                assert!(matches!(code, yerd_ipc::ErrorCode::NotFound));
                assert!(message.contains("bogus"), "{message}");
            }
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn set_enabled_config_only_branches_are_ok() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());

        assert!(matches!(set_enabled(&state, false).await, Response::Ok));
        assert!(!state.config.lock().await.dumps.enabled);

        state.config.lock().await.dumps.enabled = true;
        assert!(matches!(set_enabled(&state, true).await, Response::Ok));
        assert!(state.config.lock().await.dumps.enabled);
        let reloaded = yerd_config::Config::load(&state.config_path).unwrap();
        assert!(reloaded.dumps.enabled);
    }

    #[tokio::test]
    async fn set_port_rejects_zero_noops_same_and_moves_to_new() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());

        match set_port(&state, 0).await {
            Response::Error { code, .. } => {
                assert!(matches!(code, yerd_ipc::ErrorCode::Internal));
            }
            other => panic!("expected Error, got {other:?}"),
        }

        let cur = state.config.lock().await.dumps.port;
        assert!(matches!(set_port(&state, cur).await, Response::Ok));

        let probe = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let free = probe.local_addr().unwrap().port();
        drop(probe);
        assert_ne!(free, cur);
        assert!(matches!(set_port(&state, free).await, Response::Ok));
        assert_eq!(state.config.lock().await.dumps.port, free);
        let reloaded = yerd_config::Config::load(&state.config_path).unwrap();
        assert_eq!(reloaded.dumps.port, free);
    }

    #[tokio::test]
    async fn apply_config_writes_state_file_matching_config() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());

        assert!(matches!(
            set_feature(&state, "cache".into(), false).await,
            Response::Ok
        ));
        let sf = state.dirs.state.join("dumps").join("state.json");
        let body = std::fs::read_to_string(sf).unwrap();
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["features"]["cache"], serde_json::json!(false));
        assert_eq!(v["features"]["queries"], serde_json::json!(true));
        assert_eq!(v["enabled"], serde_json::json!(false));
        assert!(state.config.lock().await.dumps.enabled);
    }

    fn v2_line(category: &str, req: &str, seq: u32, payload: &serde_json::Value) -> String {
        serde_json::json!({
            "category": category, "ts": 0, "site": "blog", "request_id": req,
            "payload": payload, "v": 2, "seq": seq, "t_us": 5
        })
        .to_string()
    }

    fn v2_frame(category: DumpCategory, req: &str) -> IncomingFrame {
        IncomingFrame {
            v: 2,
            seq: Some(1),
            t_us: Some(5),
            ..frame_req(category, req)
        }
    }

    #[test]
    fn protocol_two_envelope_parses_and_legacy_defaults() {
        let line = v2_line("query", "abc", 3, &serde_json::json!({"sql": "select 1"}));
        let f: IncomingFrame = serde_json::from_str(&line).unwrap();
        assert_eq!((f.v, f.seq, f.t_us), (2, Some(3), Some(5)));
        let ext = f.insights_frame().unwrap();
        assert_eq!(
            (ext.request_id.as_str(), ext.site.as_str()),
            ("abc", "blog")
        );
        assert_eq!(ext.category, DumpCategory::Query);
        assert!(frame(DumpCategory::Query).insights_frame().is_none());
    }

    #[tokio::test]
    async fn a_v2_frame_reaches_pending_and_a_v1_frame_does_not() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        ingest_frame(&state.dumps, &state.insights, frame(DumpCategory::Query)).await;
        assert_eq!(state.insights.agg.lock().await.pending_len(), 0);
        ingest_frame(
            &state.dumps,
            &state.insights,
            v2_frame(DumpCategory::Query, "x"),
        )
        .await;
        assert_eq!(state.insights.agg.lock().await.pending_len(), 1);
        assert_eq!(state.dumps.ring.lock().await.list(0).events.len(), 2);
    }

    #[tokio::test]
    async fn a_v2_summary_stays_out_of_the_ring_without_the_requests_feature() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        ingest_frame(
            &state.dumps,
            &state.insights,
            v2_frame(DumpCategory::Request, "a"),
        )
        .await;
        assert_eq!(state.dumps.ring.lock().await.list(0).events.len(), 1);
        assert!(matches!(
            set_feature(&state, "requests".into(), false).await,
            Response::Ok
        ));
        ingest_frame(
            &state.dumps,
            &state.insights,
            v2_frame(DumpCategory::Request, "b"),
        )
        .await;
        ingest_frame(&state.dumps, &state.insights, frame(DumpCategory::Request)).await;
        assert_eq!(state.dumps.ring.lock().await.list(0).events.len(), 2);
        assert_eq!(state.insights.agg.lock().await.pending_len(), 2);
    }

    #[tokio::test]
    async fn loopback_v2_frames_finalise_an_injected_sample() {
        use tokio::io::AsyncWriteExt;

        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let now = now_ms();
        let id = "00000000000000010000000000000001";
        state.insights.agg.lock().await.on_sample(
            yerd_insights::ProxySample {
                id: id.to_owned(),
                site: "blog".to_owned(),
                php: "8.3".to_owned(),
                method: "GET".to_owned(),
                path: "/orders/1".to_owned(),
                script: None,
                status: 200,
                bytes: 10,
                content_type: None,
                start_us: now * 1000,
                duration_us: 20_000,
                aborted: false,
                proxy_error: false,
                traced: true,
            },
            now,
        );
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (store, insights) = (state.dumps.clone(), state.insights.clone());
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            handle_conn(stream, store, insights).await;
        });
        let mut client = TcpStream::connect(addr).await.unwrap();
        let query = v2_line(
            "query",
            id,
            1,
            &serde_json::json!({"sql": "select 1", "bindings": [], "time_ms": 2.0, "row_count": 1}),
        );
        let summary = v2_line(
            "request",
            id,
            2,
            &serde_json::json!({
                "method": "GET", "uri": "/orders/1", "status": 200, "duration_ms": 9.0,
                "start_us": now * 1000, "duration_us": 9_000, "worker_pid": 1,
                "worker_requests": 1, "route": null, "frames": 2, "frames_unsent": 0
            }),
        );
        client
            .write_all(format!("{query}\n{summary}\n").as_bytes())
            .await
            .unwrap();
        client.shutdown().await.unwrap();
        server.await.unwrap();
        let agg = state.insights.agg.lock().await;
        let rows = agg.recent_requests(
            "blog",
            yerd_ipc::InsightsRange::OneHour,
            None,
            yerd_ipc::InsightsSort::Recent,
            10,
            now,
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].queries, 1);
        assert!(rows[0].cold);
        assert_eq!(agg.pending_len(), 0);
    }

    fn dump_line(req: &str, value_len: usize) -> String {
        serde_json::json!({
            "category": "dump", "ts": 0, "site": "blog", "request_id": req,
            "payload": {"value_html": "x".repeat(value_len)}
        })
        .to_string()
    }

    async fn ring_ids_after_sending(state: &DaemonState, bytes: Vec<u8>) -> Vec<String> {
        use tokio::io::AsyncWriteExt;

        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (store, insights) = (state.dumps.clone(), state.insights.clone());
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            handle_conn(stream, store, insights).await;
        });
        let mut client = TcpStream::connect(addr).await.unwrap();
        let _ = client.write_all(&bytes).await;
        let _ = client.shutdown().await;
        server.await.unwrap();
        state
            .dumps
            .ring
            .lock()
            .await
            .list(0)
            .events
            .iter()
            .map(|e| e.request_id.clone())
            .collect()
    }

    #[tokio::test]
    async fn a_near_limit_line_and_the_next_frame_in_one_read_both_arrive() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let overhead = dump_line("big", 0).len();
        let big = dump_line("big", MAX_LINE_BYTES - 1 - overhead);
        assert_eq!(big.len() + 1, MAX_LINE_BYTES);
        let bytes = format!("{big}\n{}\n", dump_line("small", 4)).into_bytes();
        let ids = ring_ids_after_sending(&state, bytes).await;
        assert_eq!(ids, ["big", "small"]);
    }

    #[tokio::test]
    async fn an_unterminated_line_reaching_the_cap_drops_the_connection() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let unterminated = "x".repeat(MAX_LINE_BYTES + 16 * 1024);
        let bytes = format!(
            "{}\n{unterminated}\n{}\n",
            dump_line("kept", 4),
            dump_line("lost", 4)
        )
        .into_bytes();
        let ids = ring_ids_after_sending(&state, bytes).await;
        assert_eq!(ids, ["kept"]);
    }

    #[tokio::test]
    async fn run_mirrors_bound_into_the_tap() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(state_in(tmp.path()));
        let probe = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        state.config.lock().await.dumps.port = probe.local_addr().unwrap().port();
        drop(probe);
        let (tx, rx) = watch::channel(false);
        let task = tokio::spawn(run(state.clone(), rx));
        for _ in 0..200 {
            if state.insights.tap.bound() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(state.insights.tap.bound());
        assert!(state.dumps.bound.load(Ordering::Acquire));
        tx.send(true).unwrap();
        task.await.unwrap();
        assert!(!state.insights.tap.bound());
    }

    #[tokio::test]
    async fn config_changes_propagate_to_the_tap() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let tap = &state.insights.tap;
        tap.set_bound(true);
        assert!(tap.begin("blog", false).traced());

        assert!(matches!(
            set_feature(&state, "cache".into(), false).await,
            Response::Ok
        ));
        assert_eq!(&*tap.features(), "dumps,queries,jobs,views,requests,logs");
        assert!(matches!(
            set_feature(&state, "http".into(), true).await,
            Response::Ok
        ));
        assert_eq!(
            &*tap.features(),
            "dumps,queries,jobs,views,requests,logs,http"
        );

        let probe = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let free = probe.local_addr().unwrap().port();
        drop(probe);
        assert!(matches!(set_port(&state, free).await, Response::Ok));
        assert_eq!(tap.port(), free);

        assert!(matches!(set_enabled(&state, false).await, Response::Ok));
        assert!(!tap.capture());
        let ticket = tap.begin("blog", false);
        assert!(!ticket.traced());
        assert!(ticket.trace_params().is_none());
    }
}
