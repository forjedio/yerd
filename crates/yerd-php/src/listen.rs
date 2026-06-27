//! FPM listen-address abstraction.
//!
//! FPM listens on either a Unix domain socket (Unix) or a TCP loopback
//! address (Windows, and acceptable elsewhere). [`AllocatedListen::plan`]
//! is the planner-side entry: the daemon calls it before rendering the
//! pool config, then bakes the resolved address into the rendered
//! template.

#[cfg(windows)]
use std::net::Ipv4Addr;
#[cfg(windows)]
use std::net::SocketAddr;

use yerd_core::PhpVersion;
use yerd_platform::{PlatformDirs, PortBinder};

use crate::error::PhpError;

/// The generic listen-address type FPM uses, re-exported from `yerd-supervise`.
pub use yerd_supervise::Listen;

/// Result of pre-flighting a listen address for one FPM pool.
///
/// **Unix:** no socket has been created yet - FPM creates it itself when
/// it starts.
/// **Windows:** the planner briefly bound `127.0.0.1:0` via the supplied
/// `PortBinder`, captured the resolved port, and **dropped** the listener
/// before returning. The drop-then-rebind window is racy; the manager
/// retries up to `MAX_BIND_ATTEMPTS` per `ensure()` call. There is no
/// portable way to inherit an open `TcpListener` into FPM, so bind-find-port
/// + retry is the smallest correct mechanism.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllocatedListen {
    /// The address FPM will bind.
    pub listen: Listen,
}

impl AllocatedListen {
    /// Plan the listen address for `version` under `dirs`.
    ///
    /// `instance_id` is the daemon's `std::process::id()`; it disambiguates
    /// Unix socket paths across concurrent Yerd instances on the same
    /// host. On Windows it isn't embedded (the kernel assigns a unique
    /// ephemeral port), but the parameter shape stays uniform.
    ///
    /// On Unix: returns
    /// `Listen::UnixSocket(dirs.runtime/fpm-<v>-<instance>.sock)`;
    /// `binder` is ignored.
    /// On Windows: calls `binder.bind(0)`, reads the resolved port,
    /// drops the `BoundPort`, and returns
    /// `Listen::TcpLoopback(127.0.0.1:<port>)`.
    pub fn plan(
        version: PhpVersion,
        dirs: &PlatformDirs,
        instance_id: u32,
        binder: &impl PortBinder,
    ) -> Result<Self, PhpError> {
        #[cfg(unix)]
        {
            let _ = binder;
            let path = dirs
                .runtime
                .join(format!("fpm-{version}-{instance_id}.sock"));
            Ok(Self {
                listen: Listen::UnixSocket(path),
            })
        }
        #[cfg(windows)]
        {
            let _ = (version, dirs, instance_id);
            let bound = binder.bind(0).map_err(|source| PhpError::Bind { source })?;
            let port = bound.port().map_err(|source| PhpError::Bind {
                source: yerd_platform::PlatformError::Bind { port: 0, source },
            })?;
            drop(bound);
            Ok(Self {
                listen: Listen::TcpLoopback(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port)),
            })
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::path::PathBuf;

    #[cfg(unix)]
    #[test]
    fn plan_unix_uses_runtime_dir_and_instance_id() {
        struct StubBinder;
        impl PortBinder for StubBinder {
            fn bind(
                &self,
                _port: u16,
            ) -> Result<yerd_platform::BoundPort, yerd_platform::PlatformError> {
                panic!("plan() must not call bind() on Unix")
            }
            fn bind_pair(
                &self,
                _: (u16, u16),
                _: (u16, u16),
            ) -> Result<yerd_platform::PortPair, yerd_platform::PlatformError> {
                panic!("plan() must not call bind_pair() on Unix")
            }
        }

        let dirs = PlatformDirs {
            config: PathBuf::from("/c"),
            data: PathBuf::from("/d"),
            state: PathBuf::from("/s"),
            cache: PathBuf::from("/cache"),
            runtime: PathBuf::from("/run"),
        };
        let v = PhpVersion::new(8, 3);
        let plan = AllocatedListen::plan(v, &dirs, 4242, &StubBinder).unwrap();
        match plan.listen {
            Listen::UnixSocket(p) => {
                assert_eq!(p, PathBuf::from("/run/fpm-8.3-4242.sock"));
            }
            other @ Listen::TcpLoopback(_) => panic!("expected UnixSocket, got {other:?}"),
        }
    }
}
