//! Trait seams the supervisor depends on.
//!
//! Each trait is injected so the supervision driver can be tested with fakes (no
//! real process spawns, no real sockets, no real clock). Production impls of the
//! infrastructure traits live in [`crate::real`]; the [`HealthProbe`] and
//! [`Downloader`] impls are program-specific and live in the consuming crate.

use std::io;

use async_trait::async_trait;

use crate::error::{DownloadError, ExitReason};
use crate::listen::Listen;
use crate::supervisor::KillSignal;

/// Abstraction over `std::process::Command::spawn`.
///
/// `cmd` is a `std::process::Command` so the trait stays runtime-free; the
/// production impl converts to `tokio::process::Command` internally.
pub trait ProcessSpawner: Send + Sync + 'static {
    /// The handle type returned by `spawn`.
    type Child: ChildHandle;
    /// Spawn the command and return a handle the supervisor can wait on and
    /// kill.
    fn spawn(&self, cmd: std::process::Command) -> Result<Self::Child, io::Error>;
}

/// Operations the supervisor performs on a live child.
///
/// On Unix, `kill` signals the **process group** so child workers are reaped
/// along with the parent. The consumer's command builder sets `process_group(0)`
/// at spawn time so the child's PID is also the process-group ID;
/// **never refactor the Unix impl to `kill(pid)` — that would leak workers.**
/// On Windows, both signals collapse to `tokio::process::Child::kill`; workers
/// are taken down by tokio's `kill_on_drop(true)`. A Phase 2 ticket adds
/// job-object semantics via the helper.
#[async_trait]
pub trait ChildHandle: Send + 'static {
    /// PID captured once at spawn time. `tokio::process::Child::id()` returns
    /// `Option<u32>`; the real impl reads it once (before any reaping) and
    /// stashes it as `u32`.
    fn id(&self) -> u32;

    /// Non-blocking liveness probe — wraps `tokio::process::Child::try_wait`.
    fn try_wait(&mut self) -> Result<Option<ExitReason>, io::Error>;

    /// Block until the child exits. Cancel-safe (per tokio docs); the driver
    /// races this against [`HealthProbe::probe`].
    async fn wait(&mut self) -> Result<ExitReason, io::Error>;

    /// Signal the child (Unix: signals the process group).
    async fn kill(&mut self, signal: KillSignal) -> Result<(), io::Error>;
}

/// Source of `std::time::Instant`. Injected so the supervisor's elapsed-time
/// arithmetic can be deterministic in tests.
pub trait Clock: Send + Sync + 'static {
    /// Read the current monotonic instant.
    fn now(&self) -> std::time::Instant;
}

/// Readiness health-check probe.
///
/// The supervisor races this against the child's exit while in `Starting`: a
/// successful probe is the signal that the process is actually ready to serve
/// (not merely that its port is open). Implementations are program-specific —
/// FPM uses a `FastCGI` `FCGI_GET_VALUES` round-trip; a database uses a
/// protocol-level probe (e.g. Redis `PING` → `+PONG`). Test fakes return a
/// programmed outcome.
#[async_trait]
pub trait HealthProbe: Send + Sync + 'static {
    /// Probe the process at `listen`. `Ok(())` means a healthy reply was
    /// observed; any error means "not ready yet".
    async fn probe(&self, listen: &Listen) -> Result<(), io::Error>;
}

/// Bytes downloader for install artifacts.
///
/// The trait is transport-agnostic (only `async-trait`, no `reqwest`) so this
/// crate stays dependency-light; the real `reqwest`-backed impl lives in the
/// daemon (`bin/yerdd`), and tests inject a fake. SHA-256 verification of the
/// fetched bytes is the caller's job, not the downloader's.
#[async_trait]
pub trait Downloader: Send + Sync + 'static {
    /// Fetch the body bytes at `url`.
    async fn download(&self, url: &str) -> Result<Vec<u8>, DownloadError>;
}
