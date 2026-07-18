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
pub mod probe;
pub mod pure;
pub mod real;
pub mod release;
pub mod traits;
pub mod version;

pub use error::{DownloadError, ExitReason, PhpError, SpawnFailureReason};
pub use listen::{AllocatedListen, Listen};
pub use manager::{DumpExtSettings, PhpManager, PoolRunState, PoolSnapshot};
pub use pool::{ExtLoad, PoolConfig, ProcessManagerMode};
pub use probe::{probe_extension, CommandRunner, ProbeOutput, TokioCommandRunner};
pub use pure::ext_probe::{interpret_probe, ExtLoadError};
pub use real::{SystemClock, TokioChild, TokioProcessSpawner};
pub use release::{
    available_minors, current_os_arch, display_build, is_newer_build, is_safe_member,
    listing_sig_url, listing_url, patch_of, resolve_from_listing, Arch, Artifact, BinaryKind,
    Channel, Os, MIN_SUPPORTED, PHP_LISTING_BASE_URL, PHP_LISTING_SCHEMA,
};
pub use traits::{ChildHandle, Clock, Downloader, HealthProbe, ProcessSpawner};
pub use version::discover_bundled;

// Compile-time `Send + 'static` guard for the production instantiation.
const _: () = {
    const fn assert_send_static<T: Send + 'static>() {}
    assert_send_static::<PhpManager<TokioProcessSpawner, SystemClock, crate::io::FastCgiProbe>>();
};
