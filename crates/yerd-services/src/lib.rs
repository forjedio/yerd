//! Local database / cache service supervision and version management for Yerd.
//!
//! Installs prebuilt service binaries (from yerd's own hosted distribution),
//! supervises one instance per engine via the shared `yerd-supervise` state
//! machine (under the database [`yerd_supervise::supervisor::SupervisorPolicy`]),
//! and reports their live state. Mirrors `yerd-php` in structure.
//!
//! Phase 1 ships **Redis (Valkey)** end-to-end; `MySQL` / `MariaDB` / Postgres land
//! in Phase 2 (the `Service` model already enumerates them).

#![forbid(unsafe_code)]

pub mod config_render;
pub mod error;
pub mod health;
pub mod manager;
pub mod release;
pub mod service;
pub mod version;

pub use error::ServiceError;
pub use health::RedisProbe;
pub use manager::{ServiceManager, ServiceRunState, ServiceSnapshot};
pub use release::{
    artifact_url, available_versions, current_os_arch, listing_url, resolve_from_listing, Arch,
    Artifact, Os, SERVICES_BASE_URL,
};
pub use service::{Service, ServiceKind};
pub use version::{discover_installed, ServiceVersion};

// Compile-time `Send + 'static` guard for the production instantiation.
const _: () = {
    const fn assert_send_static<T: Send + 'static>() {}
    assert_send_static::<
        ServiceManager<
            yerd_supervise::TokioProcessSpawner,
            yerd_supervise::SystemClock,
            RedisProbe,
        >,
    >();
};
