//! Unprivileged TCP forwarder for Yerd's macOS privileged ports.
//!
//! `yerd elevate ports` / `yerd elevate lan` install root-owned launchd daemons
//! whose `Sockets` make launchd bind 80/443 (127.0.0.1 for loopback, 0.0.0.0
//! for LAN). launchd spawns this binary as the invoking user (`UserName`) in
//! `inetdCompatibility` `Wait` mode, which passes the *listening* socket on
//! stdin, so taking it needs no FFI and no `unsafe`. Each accepted connection
//! is relayed byte-for-byte to the daemon's rootless port on the same local
//! address it arrived on, which mirrors what the old pf `rdr` rules did.
//!
//! The library holds the argument parsing, the admission rule and the relay
//! loop so they are testable with plain loopback sockets; `main.rs` only takes
//! the listener from stdin and calls [`serve`].

#![forbid(unsafe_code)]

use std::convert::Infallible;
use std::ffi::OsString;
use std::io;
use std::net::{IpAddr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

/// Flag carrying the rootless port to forward to.
pub const TO_FLAG: &str = "--to";
/// Flag carrying the listener scope (`loopback` or `lan`).
pub const SCOPE_FLAG: &str = "--scope";
/// Flag that prints the version and exits.
pub const VERSION_FLAG: &str = "--version";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const ACCEPT_BACKOFF: Duration = Duration::from_millis(100);

/// Which launchd listener this process serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// The `127.0.0.1` listener installed by `elevate ports`.
    Loopback,
    /// The `0.0.0.0` listener installed by `elevate lan`.
    Lan,
}

impl Scope {
    /// The value used for [`SCOPE_FLAG`] on the command line.
    #[must_use]
    pub const fn as_arg(self) -> &'static str {
        match self {
            Self::Loopback => "loopback",
            Self::Lan => "lan",
        }
    }

    /// Parse a [`SCOPE_FLAG`] value.
    #[must_use]
    pub fn from_arg(value: &str) -> Option<Self> {
        match value {
            "loopback" => Some(Self::Loopback),
            "lan" => Some(Self::Lan),
            _ => None,
        }
    }
}

/// What the command line asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Print the version and exit.
    Version,
    /// Serve the launchd-provided listener.
    Forward {
        /// Listener scope.
        scope: Scope,
        /// Rootless port to relay to.
        to: u16,
    },
}

/// Command-line errors.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ArgsError {
    /// A required flag was not given.
    #[error("missing required flag {0}")]
    Missing(&'static str),
    /// A flag was given without its value.
    #[error("flag {0} needs a value")]
    MissingValue(&'static str),
    /// A flag was given twice.
    #[error("flag {0} given more than once")]
    Duplicate(&'static str),
    /// An unrecognised argument.
    #[error("unexpected argument {0:?}")]
    Unexpected(String),
    /// The scope value is not `loopback` or `lan`.
    #[error("invalid scope {0:?} (expected loopback or lan)")]
    InvalidScope(String),
    /// The port is not a non-zero `u16`.
    #[error("invalid port {0:?}")]
    InvalidPort(String),
}

/// Parse the arguments after `argv[0]`.
pub fn parse_args(args: &[OsString]) -> Result<Command, ArgsError> {
    if let [only] = args {
        if only == VERSION_FLAG {
            return Ok(Command::Version);
        }
    }
    let mut scope: Option<Scope> = None;
    let mut to: Option<u16> = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let arg = arg.to_string_lossy();
        if arg == SCOPE_FLAG {
            let value = next_value(&mut iter, SCOPE_FLAG)?;
            let parsed = Scope::from_arg(&value).ok_or(ArgsError::InvalidScope(value))?;
            if scope.replace(parsed).is_some() {
                return Err(ArgsError::Duplicate(SCOPE_FLAG));
            }
        } else if arg == TO_FLAG {
            let value = next_value(&mut iter, TO_FLAG)?;
            let port = value
                .parse::<u16>()
                .ok()
                .filter(|p| *p != 0)
                .ok_or(ArgsError::InvalidPort(value))?;
            if to.replace(port).is_some() {
                return Err(ArgsError::Duplicate(TO_FLAG));
            }
        } else {
            return Err(ArgsError::Unexpected(arg.into_owned()));
        }
    }
    Ok(Command::Forward {
        scope: scope.ok_or(ArgsError::Missing(SCOPE_FLAG))?,
        to: to.ok_or(ArgsError::Missing(TO_FLAG))?,
    })
}

fn next_value<'a>(
    iter: &mut impl Iterator<Item = &'a OsString>,
    flag: &'static str,
) -> Result<String, ArgsError> {
    iter.next()
        .map(|v| v.to_string_lossy().into_owned())
        .ok_or(ArgsError::MissingValue(flag))
}

/// The upstream for a connection accepted on `local`: the same IP, port `to`.
///
/// Using the accepted address rather than always `127.0.0.1` keeps pf's
/// behaviour: after `yerd lan disable` the daemon binds loopback only, so LAN
/// connections are refused instead of silently served.
#[must_use]
pub fn upstream_for(local: SocketAddr, to: u16) -> SocketAddr {
    SocketAddr::new(local.ip(), to)
}

/// Whether to relay a connection that arrived on `local` from `peer`.
///
/// The LAN listener is bound to the wildcard address, so it would also catch
/// loopback traffic whenever the loopback job is absent; the pf LAN rule only
/// matched the LAN IP, so loopback-destined connections are dropped in `Lan`
/// scope. Peers outside private, link-local and loopback ranges are always
/// dropped, as the pf LAN rule's source list did.
#[must_use]
pub fn admit(scope: Scope, local: IpAddr, peer: IpAddr) -> bool {
    if scope == Scope::Lan && local.is_loopback() {
        return false;
    }
    yerd_core::is_lan_source(peer)
}

/// Accept connections on `listener` forever, relaying each admitted one to
/// [`upstream_for`] on its own threads.
///
/// The listener and every accepted stream are forced into blocking mode:
/// launchd may hand over a non-blocking fd, and on Darwin accepted sockets
/// inherit `O_NONBLOCK` from the listener.
pub fn serve(listener: TcpListener, scope: Scope, to: u16) -> io::Result<Infallible> {
    listener.set_nonblocking(false)?;
    loop {
        match listener.accept() {
            Ok((stream, peer)) => handle(stream, peer, scope, to),
            Err(_) => thread::sleep(ACCEPT_BACKOFF),
        }
    }
}

fn handle(stream: TcpStream, peer: SocketAddr, scope: Scope, to: u16) {
    let Ok(local) = stream.local_addr() else {
        return;
    };
    if !admit(scope, local.ip(), peer.ip()) {
        return;
    }
    let upstream = upstream_for(local, to);
    let _ = thread::Builder::new().spawn(move || relay(stream, upstream));
}

fn relay(client: TcpStream, upstream: SocketAddr) {
    if client.set_nonblocking(false).is_err() {
        return;
    }
    let Ok(server) = TcpStream::connect_timeout(&upstream, CONNECT_TIMEOUT) else {
        return;
    };
    let _ = client.set_nodelay(true);
    let _ = server.set_nodelay(true);
    let (Ok(client_rx), Ok(server_tx)) = (client.try_clone(), server.try_clone()) else {
        return;
    };
    let Ok(back) = thread::Builder::new().spawn(move || pipe(server, client)) else {
        return;
    };
    pipe(client_rx, server_tx);
    let _ = back.join();
}

fn pipe(mut from: TcpStream, mut to: TcpStream) {
    if io::copy(&mut from, &mut to).is_ok() {
        let _ = to.shutdown(Shutdown::Write);
    } else {
        let _ = from.shutdown(Shutdown::Both);
        let _ = to.shutdown(Shutdown::Both);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, SocketAddrV4};

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn parse_args_table() {
        let cases: &[(&[&str], Result<Command, ArgsError>)] = &[
            (&["--version"], Ok(Command::Version)),
            (
                &["--scope", "lan", "--to", "8080"],
                Ok(Command::Forward {
                    scope: Scope::Lan,
                    to: 8080,
                }),
            ),
            (
                &["--to", "8443", "--scope", "loopback"],
                Ok(Command::Forward {
                    scope: Scope::Loopback,
                    to: 8443,
                }),
            ),
            (&["--to", "8080"], Err(ArgsError::Missing(SCOPE_FLAG))),
            (&["--scope", "lan"], Err(ArgsError::Missing(TO_FLAG))),
            (
                &["--scope", "wan", "--to", "8080"],
                Err(ArgsError::InvalidScope("wan".into())),
            ),
            (
                &["--scope", "lan", "--to", "0"],
                Err(ArgsError::InvalidPort("0".into())),
            ),
            (
                &["--scope", "lan", "--to", "70000"],
                Err(ArgsError::InvalidPort("70000".into())),
            ),
            (
                &["--scope", "lan", "--to"],
                Err(ArgsError::MissingValue(TO_FLAG)),
            ),
            (
                &["--scope", "lan", "--to", "8080", "--bogus"],
                Err(ArgsError::Unexpected("--bogus".into())),
            ),
            (
                &["--scope", "lan", "--to", "8080", "--to", "9090"],
                Err(ArgsError::Duplicate(TO_FLAG)),
            ),
            (
                &["--version", "--scope", "lan"],
                Err(ArgsError::Unexpected("--version".into())),
            ),
            (&[], Err(ArgsError::Missing(SCOPE_FLAG))),
        ];
        for (args, want) in cases {
            assert_eq!(&parse_args(&os(args)), want, "args {args:?}");
        }
    }

    #[test]
    fn scope_arg_round_trips() {
        for scope in [Scope::Loopback, Scope::Lan] {
            assert_eq!(Scope::from_arg(scope.as_arg()), Some(scope));
        }
    }

    #[test]
    fn upstream_keeps_local_ip() {
        let local = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 5), 80));
        assert_eq!(
            upstream_for(local, 8080),
            "192.168.1.5:8080".parse::<SocketAddr>().unwrap()
        );
    }

    #[test]
    fn admit_table() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        let cases = [
            (Scope::Loopback, "127.0.0.1", "127.0.0.1", true),
            (Scope::Lan, "127.0.0.1", "127.0.0.1", false),
            (Scope::Lan, "192.168.1.5", "192.168.1.9", true),
            (Scope::Lan, "192.168.1.5", "10.0.0.7", true),
            (Scope::Lan, "192.168.1.5", "8.8.8.8", false),
            (Scope::Loopback, "127.0.0.1", "8.8.8.8", false),
        ];
        for (scope, local, peer, want) in cases {
            assert_eq!(
                admit(scope, ip(local), ip(peer)),
                want,
                "{scope:?} {local} {peer}"
            );
        }
    }
}
