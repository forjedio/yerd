//! Insights for Yerd: per-site request performance and query problems.
//!
//! The proxy times every PHP request of an Insights-enabled site and the
//! `yerd-php-ext` extension narrates it (queries, dumps, logs, cache, http),
//! joined by a proxy-supplied request id. This crate turns those inputs into
//! finalised requests, per-minute rollups, detected issues and the wire types
//! the GUI reads.
//!
//! ## Purity boundary
//!
//! Everything in [`pure`] is sync, clock-free and does no I/O: every method
//! that depends on time takes `now` from the caller. The on-disk store in
//! [`io`] is the synchronous std-fs edge; the daemon runs it on a blocking
//! thread, so the crate never depends on an async runtime.

#![forbid(unsafe_code)]

pub mod error;
pub mod io;
pub mod pure;

pub use error::InsightsError;
pub use io::{LoadedSite, Store};
pub use pure::aggregator::{Aggregator, FlushBatch, HourBatch, TickOutcome};
pub use pure::contract::{ExtFrame, ExtRoute, ExtSummary, ProxySample};
pub use pure::dict::{Dict, DictEntry};
pub use pure::hist::LatencyHist;
pub use pure::issues::{IssueHit, IssueKind, QueryEvent, Thresholds};
pub use pure::model::{DetailEvent, FinalRequest, FpUse, RequestDetail, RequestIssue};
pub use pure::rollup::MinuteRollup;
pub use pure::route::{RouteKey, RouteKind};
