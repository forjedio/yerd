//! Dump-telemetry data model shared between the daemon, the GUI, and
//! (indirectly) the `yerd-php-ext` extension.
//!
//! The extension ships per-request telemetry frames to the daemon's
//! loopback dump server; the daemon buffers them as [`DumpEvent`]s and
//! serves them to the GUI over IPC. The daemon deliberately treats each
//! event's [`DumpEvent::payload`] as an **opaque** JSON value: it never
//! interprets the category-specific fields, so the extension's payload
//! schema can evolve without daemon changes. The GUI renders the payload
//! per [`DumpCategory`].
//!
//! Wire shapes are pinned in `tests/wire_stability.rs`.

use serde::{Deserialize, Serialize};

/// The category of a captured telemetry frame — one per GUI tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum DumpCategory {
    /// A `dump()` / `dd()` / `ddd()` call.
    Dump,
    /// An Eloquent / PDO database query.
    Query,
    /// A dispatched queue job.
    Job,
    /// A rendered Blade view.
    View,
    /// An HTTP request summary.
    Request,
    /// A log write.
    Log,
    /// A cache hit / miss / write / forget.
    Cache,
}

/// A single buffered telemetry event.
///
/// `id` and `pinned` are assigned by the daemon; the remaining fields come
/// from the extension's frame. `payload` is opaque to the daemon — see the
/// module docs.
///
/// Not `Eq` because `payload` is a [`serde_json::Value`] (floats); `PartialEq`
/// is enough for the wire-stability round-trip assertions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DumpEvent {
    /// Monotonic id assigned by the daemon; clients page with `since_id`.
    pub id: u64,
    /// Which tab this event belongs to.
    pub category: DumpCategory,
    /// Capture time, Unix epoch milliseconds (from the extension).
    pub ts_ms: u64,
    /// The originating `.test` site (e.g. `"blog.test"`); may be empty.
    pub site: String,
    /// Stable per-PHP-request id, so the GUI can group rows by request.
    pub request_id: String,
    /// Category-specific payload, opaque to the daemon. See `architecture.md`.
    pub payload: serde_json::Value,
    /// Whether the user pinned this event (survives eviction / clear).
    pub pinned: bool,
}

/// Per-category counts of the events currently buffered in the daemon's ring.
///
/// The GUI sums these for the "All" tab.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DumpCounts {
    /// Number of buffered `dump` events.
    pub dumps: u64,
    /// Number of buffered `query` events.
    pub queries: u64,
    /// Number of buffered `job` events.
    pub jobs: u64,
    /// Number of buffered `view` events.
    pub views: u64,
    /// Number of buffered `request` events.
    pub requests: u64,
    /// Number of buffered `log` events.
    pub logs: u64,
    /// Number of buffered `cache` events.
    pub cache: u64,
}

impl DumpCounts {
    /// Add one to the count for `category`.
    pub fn increment(&mut self, category: DumpCategory) {
        match category {
            DumpCategory::Dump => self.dumps += 1,
            DumpCategory::Query => self.queries += 1,
            DumpCategory::Job => self.jobs += 1,
            DumpCategory::View => self.views += 1,
            DumpCategory::Request => self.requests += 1,
            DumpCategory::Log => self.logs += 1,
            DumpCategory::Cache => self.cache += 1,
        }
    }
}

/// Whether a matching extension `.so` is present for an installed PHP version.
///
/// This is a yerd-side fact ("a matching artifact is present and was wired to
/// `-d zend_extension`") — not proof that FPM actually `dlopen`'d it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DumpExtStatus {
    /// The installed PHP minor (e.g. `8.3`).
    pub version: yerd_core::PhpVersion,
    /// Whether a matching extension artifact is present for it.
    pub present: bool,
}
