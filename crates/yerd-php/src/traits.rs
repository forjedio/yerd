//! Re-export of the shared supervision trait seams.
//!
//! These moved to `yerd-supervise`; re-exported here so existing
//! `crate::traits::*` paths and the `yerd_php` public API are unchanged.

pub use yerd_supervise::traits::*;
