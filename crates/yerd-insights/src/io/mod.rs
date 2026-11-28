//! The std-fs edge of the Insights crate. Synchronous by design: the daemon
//! calls it from `spawn_blocking`, so this crate stays runtime-free.

pub mod store;

pub use store::{LoadedSite, Store};
