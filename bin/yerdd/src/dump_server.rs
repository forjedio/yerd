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
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{watch, Mutex, Notify};

use yerd_config::DumpsSection;
use yerd_core::PhpVersion;
use yerd_ipc::{DumpCategory, DumpCounts, DumpEvent, DumpExtStatus, Response};
use yerd_platform::PlatformDirs;

use crate::state::DaemonState;

/// Maximum number of events retained in the ring before the oldest non-pinned
/// ones are evicted.
const RING_CAP: usize = 2000;
/// Maximum number of pinned events (well below [`RING_CAP`] so eviction always
/// finds a non-pinned victim).
const PINNED_CAP: usize = 200;
/// How many recently-removed ids to remember, so an incrementally-polling client
/// can reconcile deletes/evictions it still holds.
const REMOVED_LOG_CAP: usize = 4096;
/// Hard cap on a single newline-delimited frame; longer lines drop the
/// connection (the extension truncates payloads well below this).
const MAX_LINE_BYTES: usize = 256 * 1024;

/// The telemetry feature keys mirrored into the extension's state file. An absent
/// key in the config map means "on".
const FEATURES: &[&str] = &[
    "dumps", "queries", "jobs", "views", "requests", "logs", "cache",
];

/// Shared dump-telemetry state: the ring plus a rebind signal and a bound flag.
pub struct DumpStore {
    ring: Mutex<DumpRing>,
    /// Notified when the configured port changes so the server rebinds.
    rebind: Notify,
    /// Whether the server is currently bound and accepting.
    bound: AtomicBool,
}

impl DumpStore {
    /// A fresh, empty store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            ring: Mutex::new(DumpRing::new()),
            rebind: Notify::new(),
            bound: AtomicBool::new(false),
        }
    }

    /// Signal the server task to rebind (after a port change).
    pub fn request_rebind(&self) {
        self.rebind.notify_one();
    }
}

impl Default for DumpStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Bounded in-memory buffer of [`DumpEvent`]s.
struct DumpRing {
    events: VecDeque<DumpEvent>,
    removed: VecDeque<u64>,
    next_id: u64,
    pinned_count: usize,
}

impl DumpRing {
    const fn new() -> Self {
        Self {
            events: VecDeque::new(),
            removed: VecDeque::new(),
            next_id: 0,
            pinned_count: 0,
        }
    }

    fn push(&mut self, frame: IncomingFrame, now_ms: u64) {
        self.next_id = self.next_id.saturating_add(1);
        let ts_ms = if frame.ts != 0 { frame.ts } else { now_ms };
        self.events.push_back(DumpEvent {
            id: self.next_id,
            category: frame.category,
            ts_ms,
            site: frame.site,
            request_id: frame.request_id,
            payload: frame.payload,
            pinned: false,
        });
        while self.events.len() > RING_CAP {
            // Evict the oldest non-pinned event. With PINNED_CAP < RING_CAP there
            // is always one to find; the `break` is a defensive backstop.
            let Some(idx) = self.events.iter().position(|e| !e.pinned) else {
                break;
            };
            if let Some(ev) = self.events.remove(idx) {
                self.note_removed(ev.id);
            }
        }
    }

    fn note_removed(&mut self, id: u64) {
        self.removed.push_back(id);
        while self.removed.len() > REMOVED_LOG_CAP {
            self.removed.pop_front();
        }
    }

    fn delete(&mut self, id: u64) {
        if let Some(idx) = self.events.iter().position(|e| e.id == id) {
            if let Some(ev) = self.events.remove(idx) {
                if ev.pinned {
                    self.pinned_count = self.pinned_count.saturating_sub(1);
                }
                self.note_removed(id);
            }
        }
    }

    fn set_pinned(&mut self, id: u64, pinned: bool) {
        let Some(idx) = self.events.iter().position(|e| e.id == id) else {
            return;
        };
        let was = matches!(self.events.get(idx), Some(e) if e.pinned);
        if was == pinned {
            return;
        }
        if pinned && self.pinned_count >= PINNED_CAP {
            return;
        }
        if let Some(ev) = self.events.get_mut(idx) {
            ev.pinned = pinned;
        }
        if pinned {
            self.pinned_count = self.pinned_count.saturating_add(1);
        } else {
            self.pinned_count = self.pinned_count.saturating_sub(1);
        }
    }

    fn clear(&mut self) {
        for ev in self.events.drain(..) {
            self.removed.push_back(ev.id);
        }
        while self.removed.len() > REMOVED_LOG_CAP {
            self.removed.pop_front();
        }
        self.pinned_count = 0;
    }

    fn counts(&self) -> DumpCounts {
        let mut c = DumpCounts::default();
        for e in &self.events {
            c.increment(e.category);
        }
        c
    }

    /// Events newer than `since_id`, the removed ids the client may still hold,
    /// the current counts, and the cursor to send next (the highest id ever
    /// assigned, so a client always advances past removed-but-unseen ids).
    fn list(&self, since_id: u64) -> (Vec<DumpEvent>, Vec<u64>, DumpCounts, u64) {
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
        (events, removed, self.counts(), self.next_id)
    }
}

/// A frame as sent by the extension. `ts` is epoch milliseconds; 0 (or absent)
/// means "stamp on receipt".
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
                state.dumps.bound.store(true, Ordering::Release);
                tracing::info!(port, "dump server bound");
                l
            }
            Err(e) => {
                state.dumps.bound.store(false, Ordering::Release);
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
                    state.dumps.bound.store(false, Ordering::Release);
                    return;
                }
                () = state.dumps.rebind.notified() => break,
                accepted = listener.accept() => {
                    match accepted {
                        Ok((stream, _peer)) => {
                            let store = state.dumps.clone();
                            tokio::spawn(handle_conn(stream, store));
                        }
                        Err(e) => tracing::debug!(error = %e, "dump server accept failed"),
                    }
                }
            }
        }
        state.dumps.bound.store(false, Ordering::Release);
    }
}

/// Read newline-delimited JSON frames from one connection until EOF, pushing each
/// valid frame into the ring. Bounded: an over-long line drops the connection;
/// malformed lines are skipped.
async fn handle_conn(stream: TcpStream, store: Arc<DumpStore>) {
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
        if pending.len().saturating_add(slice.len()) > MAX_LINE_BYTES {
            // A single frame must fit the cap; otherwise treat the peer as hostile.
            return;
        }
        pending.extend_from_slice(slice);
        while let Some(pos) = pending.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = pending.drain(..pos).collect();
            pending.drain(..1); // drop the '\n'
            if line.is_empty() {
                continue;
            }
            if let Ok(frame) = serde_json::from_slice::<IncomingFrame>(&line) {
                let mut ring = store.ring.lock().await;
                ring.push(frame, now_ms());
            }
        }
    }
}

// ---------- IPC handlers (called from `ipc_server::dispatch`) ----------

/// Page the ring for `ListDumps`.
pub async fn list(state: &DaemonState, since_id: u64) -> Response {
    let ring = state.dumps.ring.lock().await;
    let (events, removed_ids, counts, latest_id) = ring.list(since_id);
    Response::Dumps {
        events,
        removed_ids,
        counts,
        latest_id,
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

/// Pin or unpin one event.
pub async fn pin(state: &DaemonState, id: u64, pinned: bool) -> Response {
    state.dumps.ring.lock().await.set_pinned(id, pinned);
    Response::Ok
}

/// Report dump-server status.
pub async fn status(state: &DaemonState) -> Response {
    let (enabled, port, features) = {
        let c = state.config.lock().await;
        let features = FEATURES
            .iter()
            .map(|&f| ((*f).to_string(), c.dumps.features.get(f).copied().unwrap_or(true)))
            .collect();
        (c.dumps.enabled, c.dumps.port, features)
    };
    let counts = state.dumps.ring.lock().await.counts();
    let running = state.dumps.bound.load(Ordering::Acquire);
    let extensions = extension_presence(&state.dirs);
    Response::DumpsStatus {
        enabled,
        port,
        running,
        extensions,
        counts,
        features,
    }
}

/// Set the antenna (enabled) flag: persist config and rewrite the state file.
pub async fn set_enabled(state: &DaemonState, enabled: bool) -> Response {
    apply_config(state, |d| d.enabled = enabled).await
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
    if let Err(e) = write_state_file(&state.dirs, &snapshot) {
        tracing::warn!(error = %e, "failed to write dump state file");
    }
    Response::Ok
}

/// Mirror of the `[dumps]` config the extension reads each request.
#[derive(Serialize)]
struct StateFile<'a> {
    enabled: bool,
    port: u16,
    features: std::collections::BTreeMap<&'a str, bool>,
}

/// Write `{state}/dumps/state.json` atomically (temp-then-rename). The features
/// map is fully resolved (every known key present) so the extension needs no
/// default logic.
pub fn write_state_file(dirs: &PlatformDirs, dumps: &DumpsSection) -> std::io::Result<()> {
    let dir = dirs.state.join("dumps");
    std::fs::create_dir_all(&dir)?;
    let features = FEATURES
        .iter()
        .map(|&f| (f, dumps.features.get(f).copied().unwrap_or(true)))
        .collect();
    let body = StateFile {
        enabled: dumps.enabled,
        port: dumps.port,
        features,
    };
    let json = serde_json::to_vec(&body).map_err(std::io::Error::other)?;
    let tmp = dir.join("state.json.tmp");
    let final_path = dir.join("state.json");
    std::fs::write(&tmp, &json)?;
    std::fs::rename(&tmp, &final_path)?;
    Ok(())
}

/// Per-installed-version extension presence: scan `{data}/php` for `php-X.Y`
/// installs and check whether a matching `.so` exists in the sibling
/// `{data}/php-ext/php-X.Y/yerd-dump.so` tree.
fn extension_presence(dirs: &PlatformDirs) -> Vec<DumpExtStatus> {
    let mut out = Vec::new();
    let php_dir = dirs.data.join("php");
    let Ok(entries) = std::fs::read_dir(&php_dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(ver_str) = name.strip_prefix("php-") else {
            continue;
        };
        let Ok(version) = PhpVersion::from_str(ver_str) else {
            continue;
        };
        let so = dirs
            .data
            .join("php-ext")
            .join(name)
            .join("yerd-dump.so");
        out.push(DumpExtStatus {
            version,
            present: so.is_file(),
        });
    }
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
        IncomingFrame {
            category,
            ts: 0,
            site: "blog.test".into(),
            request_id: "req".into(),
            payload: serde_json::json!({"x": 1}),
        }
    }

    #[test]
    fn push_assigns_monotonic_ids_and_counts() {
        let mut ring = DumpRing::new();
        ring.push(frame(DumpCategory::Query), 100);
        ring.push(frame(DumpCategory::Query), 100);
        ring.push(frame(DumpCategory::Dump), 100);
        let (events, removed, counts, latest) = ring.list(0);
        assert_eq!(events.len(), 3);
        assert!(removed.is_empty());
        assert_eq!(counts.queries, 2);
        assert_eq!(counts.dumps, 1);
        assert_eq!(latest, 3);
        assert_eq!(events.first().map(|e| e.id), Some(1));
    }

    #[test]
    fn list_since_filters_and_reports_removed() {
        let mut ring = DumpRing::new();
        ring.push(frame(DumpCategory::Query), 100); // id 1
        ring.push(frame(DumpCategory::Query), 100); // id 2
        // Client has seen up to id 2.
        ring.delete(1);
        let (events, removed, _counts, latest) = ring.list(2);
        assert!(events.is_empty(), "no events newer than id 2");
        assert_eq!(removed, vec![1], "id 1 was deleted and is <= since_id");
        assert_eq!(latest, 2);
    }

    #[test]
    fn pinned_event_survives_eviction() {
        let mut ring = DumpRing::new();
        ring.push(frame(DumpCategory::Dump), 100); // id 1
        ring.set_pinned(1, true);
        for _ in 0..RING_CAP + 10 {
            ring.push(frame(DumpCategory::Query), 100);
        }
        let (events, _removed, _counts, _latest) = ring.list(0);
        assert!(
            events.iter().any(|e| e.id == 1 && e.pinned),
            "pinned id 1 must survive eviction"
        );
        assert!(events.len() <= RING_CAP);
    }

    #[test]
    fn clear_drops_everything_including_pinned() {
        let mut ring = DumpRing::new();
        ring.push(frame(DumpCategory::Dump), 100); // id 1
        ring.set_pinned(1, true);
        ring.clear();
        // A client that had seen up to id 1 learns it was removed.
        let (events, removed, counts, _latest) = ring.list(1);
        assert!(events.is_empty());
        assert_eq!(removed, vec![1]);
        assert_eq!(counts.dumps, 0);
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
        assert_eq!(v["enabled"], serde_json::json!(true));
        assert_eq!(v["port"], serde_json::json!(2304));
        assert_eq!(v["features"]["queries"], serde_json::json!(false));
        assert_eq!(v["features"]["dumps"], serde_json::json!(true));
    }
}
