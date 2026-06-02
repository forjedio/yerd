//! I/O helpers that aren't part of the supervisor proper.
//!
//! These do filesystem and socket work and are therefore not `pure/`.

pub mod atomic_write;
pub mod fastcgi_probe;

pub use fastcgi_probe::FastCgiProbe;
