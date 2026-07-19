//! Local database / cache service supervision and version management for Yerd.
//!
//! Installs prebuilt service binaries (from yerd's own hosted distribution),
//! supervises one instance per engine via the shared `yerd-supervise` state
//! machine (under the database [`yerd_supervise::supervisor::SupervisorPolicy`]),
//! and reports their live state. Mirrors `yerd-php` in structure.
//!
//! The engines - **Redis (Valkey)**, `MySQL`, `MariaDB`, Postgres, and
//! Meilisearch - are
//! implemented end-to-end (supervision, datadir init, config rendering, health
//! probing, and SQL database administration for the three SQL engines). Whether a
//! given engine/version installs depends only on whether a prebuilt build is
//! published in the hosted listing for the platform.

#![forbid(unsafe_code)]

pub mod config_render;
pub mod database;
pub mod error;
pub mod health;
pub mod manager;
pub mod port;
pub mod release;
pub mod service;
pub mod version;

pub use database::DbNameError;
pub use error::ServiceError;
pub use health::{MeilisearchProbe, ReadinessProbe, RedisProbe, ServiceProbes, TcpConnectProbe};
pub use manager::{ServiceManager, ServiceRunState, ServiceSnapshot};
pub use port::candidate_ports;
pub use release::{
    artifact_url, available_versions, current_os_arch, listing_url, platform_token,
    resolve_from_listing, Arch, Artifact, Os, LISTING_SCHEMA, SERVICES_BASE_URL,
};
pub use service::{
    DatadirScope, LaunchContext, LaunchPlan, MariaDb, Meilisearch, Multiplicity, MySql, Postgres,
    ReadinessKind, Redis, Reverb, ServiceDefinition, ServiceKind, ServiceRegistry, SqlEngine,
};
pub use version::{discover_installed, ServiceVersion};

// Compile-time `Send + 'static` guard for the production instantiation.
const _: () = {
    const fn assert_send_static<T: Send + 'static>() {}
    assert_send_static::<
        ServiceManager<
            yerd_supervise::TokioProcessSpawner,
            yerd_supervise::SystemClock,
            ServiceProbes,
        >,
    >();
};
