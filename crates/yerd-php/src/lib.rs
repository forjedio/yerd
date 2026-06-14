//! PHP-FPM pool supervision and version management for Yerd.
//!
//! Supervises one PHP-FPM pool per installed PHP version and discovers the
//! bundled installs available to use.

#![forbid(unsafe_code)]

pub mod error;
pub mod io;
pub mod listen;
pub mod manager;
pub mod pool;
pub mod pure;
pub mod real;
pub mod release;
pub mod traits;
pub mod version;

pub use error::{DownloadError, ExitReason, PhpError, SpawnFailureReason};
pub use listen::{AllocatedListen, Listen};
pub use manager::{DumpExtSettings, PhpManager, PoolRunState, PoolSnapshot};
pub use pool::{PoolConfig, ProcessManagerMode};
pub use real::{SystemClock, TokioChild, TokioProcessSpawner};
pub use release::{
    artifact_url, available_minors, current_os_arch, is_newer, is_safe_member, listing_url,
    patch_of, resolve_from_listing, Arch, Artifact, BinaryKind, Os,
};
pub use traits::{ChildHandle, Clock, Downloader, HealthProbe, ProcessSpawner};
pub use version::discover_bundled;

// Compile-time `Send + 'static` guard for the production instantiation.
const _: () = {
    const fn assert_send_static<T: Send + 'static>() {}
    assert_send_static::<PhpManager<TokioProcessSpawner, SystemClock, crate::io::FastCgiProbe>>();
};
