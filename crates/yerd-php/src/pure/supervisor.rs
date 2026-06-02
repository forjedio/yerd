//! Re-export of the shared pure supervision state machine.
//!
//! The state machine moved to `yerd-supervise` so `yerd-services` can drive it
//! too; it is re-exported here so existing `crate::pure::supervisor::*` paths
//! (and the `yerd_php` public API) are unchanged.

pub use yerd_supervise::supervisor::*;
