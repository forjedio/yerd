//! `ResolverInstaller` trait.

use std::net::SocketAddr;

use crate::PlatformError;

/// OS resolver redirection abstraction.
///
/// `install` and `uninstall` always return `NeedsHelper` in Phase 1; the
/// daemon materialises a `HelperInvocation::InstallResolver` /
/// `UninstallResolver` from the same `tld` and `addr` it passed here.
/// `is_installed` reads public files and is unprivileged.
///
/// Both `uninstall(tld)` for an absent TLD and `is_installed(tld)` for an
/// absent TLD return `Ok(())` / `Ok(false)` - idempotent.
pub trait ResolverInstaller {
    /// Request resolver redirection for `tld` to `addr`.
    ///
    /// `addr` is the IP+port the OS resolver should forward to. Phase 1
    /// daemon always passes `127.0.0.1:<port>`; the trait accepts
    /// `SocketAddr` so v2 can move the DNS responder elsewhere without a
    /// breaking change.
    fn install(&self, tld: &str, addr: SocketAddr) -> Result<(), PlatformError>;

    /// Request resolver redirection removal for `tld`. Idempotent.
    fn uninstall(&self, tld: &str) -> Result<(), PlatformError>;

    /// Probe whether the OS resolver is currently redirecting `tld` to Yerd at
    /// `addr`. Idempotent.
    ///
    /// `addr` is the IP+port the resolver should forward to (the daemon's live
    /// DNS responder). Implementations verify the on-disk config points there -
    /// a stale file aimed elsewhere (e.g. a Valet/Herd leftover on `:53`) must
    /// report `false` so the redirect gets (re)installed.
    fn is_installed(&self, tld: &str, addr: SocketAddr) -> Result<bool, PlatformError>;
}
