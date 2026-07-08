//! Pure (synchronous, runtime-free, I/O-free) helpers.
//!
//! Mirrors `yerd-platform::pure`: everything here is decision logic that
//! can be unit-tested in-memory. The driver in `manager.rs` and the I/O
//! helpers under `io/` are the only places that touch `tokio` / sockets
//! / the filesystem.

pub mod env_scrub;
pub mod ext_probe;
pub mod fpm_conf;
pub mod supervisor;
