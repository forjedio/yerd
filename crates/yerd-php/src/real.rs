//! Re-export of the shared production `Clock` / `ProcessSpawner` impls.
//!
//! These moved to `yerd-supervise`; re-exported here so existing
//! `crate::real::*` paths and the `yerd_php` public API are unchanged.

pub use yerd_supervise::real::{SystemClock, TokioChild, TokioProcessSpawner};
