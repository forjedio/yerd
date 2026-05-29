//! Trait seams the manager depends on.
//!
//! Each trait is injected so the supervision driver can be tested with
//! fakes (no real process spawns, no real sockets, no real clock).
//! Production impls live in [`crate::real`] and [`crate::io::fastcgi_probe`].

use std::io;

use async_trait::async_trait;

use crate::error::ExitReason;
use crate::listen::Listen;
use crate::pure::supervisor::KillSignal;

/// Abstraction over `std::process::Command::spawn`.
///
/// `cmd` is a `std::process::Command` so the trait stays runtime-free; the
/// production impl converts to `tokio::process::Command` internally.
pub trait ProcessSpawner: Send + Sync + 'static {
    /// The handle type returned by `spawn`.
    type Child: ChildHandle;
    /// Spawn the command and return a handle the supervisor can wait on
    /// and kill.
    fn spawn(&self, cmd: std::process::Command) -> Result<Self::Child, io::Error>;
}

/// Operations the supervisor performs on a live child.
///
/// On Unix, `kill` signals the **process group** so FPM workers are reaped
/// along with the parent. The production spawner sets `process_group(0)`
/// at spawn time so the child's PID is also the process-group ID;
/// **never refactor the Unix impl to `kill(pid)` — that would leak workers.**
/// On Windows, both signals collapse to `tokio::process::Child::kill`;
/// FPM workers are taken down by tokio's `kill_on_drop(true)`. A Phase 2
/// ticket adds job-object semantics via the helper.
#[async_trait]
pub trait ChildHandle: Send + 'static {
    /// PID captured once at spawn time. `tokio::process::Child::id()`
    /// returns `Option<u32>`; the real impl reads it once (before any
    /// reaping) and stashes it as `u32`.
    fn id(&self) -> u32;

    /// Non-blocking liveness probe — wraps `tokio::process::Child::try_wait`.
    fn try_wait(&mut self) -> Result<Option<ExitReason>, io::Error>;

    /// Block until the child exits. Cancel-safe (per tokio docs); the
    /// driver races this against [`HealthProbe::probe`].
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

/// FastCGI health-check probe.
///
/// The production impl in [`crate::io::fastcgi_probe::FastCgiProbe`] opens a
/// TCP/Unix stream, sends a `FCGI_GET_VALUES` record, and reads back any
/// record-shaped reply. Test fakes can return a programmed outcome.
#[async_trait]
pub trait HealthProbe: Send + Sync + 'static {
    /// Probe the FPM pool at `listen`. `Ok(())` means a healthy reply was
    /// observed; any error means "not healthy yet".
    async fn probe(&self, listen: &Listen) -> Result<(), io::Error>;
}

/// Bytes downloader for PHP install artifacts.
///
/// The trait is transport-agnostic (only `async-trait`, no `reqwest`) so
/// `yerd-php` stays dependency-light; the real `reqwest`-backed impl lives in
/// the daemon (`bin/yerdd`), and tests inject a fake. SHA-256 verification of
/// the fetched bytes is the caller's job, not the downloader's.
#[async_trait]
pub trait Downloader: Send + Sync + 'static {
    /// Fetch the body bytes at `url`.
    async fn download(&self, url: &str) -> Result<Vec<u8>, crate::error::DownloadError>;
}
