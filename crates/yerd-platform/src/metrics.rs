//! `SystemMetrics` trait - best-effort process/system resource readings.
//!
//! Unlike the other traits in this crate ([`crate::TrustStore`],
//! [`crate::ResolverInstaller`], [`crate::PortBinder`]) which return
//! `Result<_, PlatformError>`, metrics are **best-effort and return `Option`**:
//! `None` covers both "this OS is not supported" (macOS/Windows in Phase 1) and
//! "a transient read failed". Callers that surface metrics (e.g. `yerd status`)
//! treat the two cases identically - show nothing - so collapsing them keeps the
//! call sites simple. The actual decoding lives in pure, table-tested parsers
//! ([`crate::pure::proc_metrics`]); the OS impls only do the file reads.

/// Best-effort process- and system-level resource metrics.
pub trait SystemMetrics {
    /// Resident set size (physical memory) of process `pid`, in bytes.
    ///
    /// `None` when the process is gone, unreadable, or the OS is unsupported.
    fn rss_bytes(&self, pid: u32) -> Option<u64>;

    /// System load average over the last 1, 5, and 15 minutes.
    ///
    /// `None` on platforms without a cheap load-average source.
    fn load_average(&self) -> Option<[f64; 3]>;
}
