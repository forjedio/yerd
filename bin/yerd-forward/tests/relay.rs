//! End-to-end relay behaviour of [`yerd_forward::serve`] over real loopback
//! sockets.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use yerd_forward::{serve, Scope};

const WAIT: Duration = Duration::from_secs(5);

fn spawn_forwarder(scope: Scope, to: u16, nonblocking: bool) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    listener.set_nonblocking(nonblocking).unwrap();
    thread::spawn(move || serve(listener, scope, to));
    addr
}

fn spawn_echo(delay: Duration) -> (u16, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let accepted = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&accepted);
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            counter.fetch_add(1, Ordering::SeqCst);
            thread::spawn(move || {
                let mut buf = Vec::new();
                stream.read_to_end(&mut buf).unwrap();
                thread::sleep(delay);
                stream.write_all(&buf).unwrap();
                stream.shutdown(Shutdown::Write).unwrap();
            });
        }
    });
    (port, accepted)
}

fn round_trip(addr: SocketAddr, payload: &[u8]) -> Vec<u8> {
    let mut client = TcpStream::connect_timeout(&addr, WAIT).unwrap();
    client.set_read_timeout(Some(WAIT)).unwrap();
    client.write_all(payload).unwrap();
    client.shutdown(Shutdown::Write).unwrap();
    let mut out = Vec::new();
    client.read_to_end(&mut out).unwrap();
    out
}

#[test]
fn relays_both_directions_with_half_close() {
    let (port, _) = spawn_echo(Duration::ZERO);
    let addr = spawn_forwarder(Scope::Loopback, port, false);
    assert_eq!(round_trip(addr, b"hello yerd"), b"hello yerd");
}

#[test]
fn closed_upstream_closes_client_promptly() {
    let unused = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = unused.local_addr().unwrap().port();
    drop(unused);
    let addr = spawn_forwarder(Scope::Loopback, port, false);
    let mut client = TcpStream::connect_timeout(&addr, WAIT).unwrap();
    client.set_read_timeout(Some(WAIT)).unwrap();
    let mut buf = [0_u8; 16];
    assert_closed(&client.read(&mut buf));
}

#[test]
fn serves_concurrent_clients() {
    let (port, _) = spawn_echo(Duration::from_millis(50));
    let addr = spawn_forwarder(Scope::Loopback, port, false);
    let handles: Vec<_> = (0..10)
        .map(|i| {
            thread::spawn(move || {
                let payload = format!("client-{i}");
                assert_eq!(round_trip(addr, payload.as_bytes()), payload.as_bytes());
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
}

#[test]
fn nonblocking_listener_still_relays_slow_upstream() {
    let (port, _) = spawn_echo(Duration::from_millis(200));
    let addr = spawn_forwarder(Scope::Loopback, port, true);
    assert_eq!(round_trip(addr, b"slow reply"), b"slow reply");
}

#[test]
fn lan_scope_drops_loopback_destined_connections() {
    let (port, accepted) = spawn_echo(Duration::ZERO);
    let addr = spawn_forwarder(Scope::Lan, port, false);
    let mut client = TcpStream::connect_timeout(&addr, WAIT).unwrap();
    client.set_read_timeout(Some(WAIT)).unwrap();
    let _ = client.write_all(b"ping");
    let mut buf = [0_u8; 16];
    assert_closed(&client.read(&mut buf));
    assert_eq!(accepted.load(Ordering::SeqCst), 0);
}

/// The forwarder closed the connection: EOF or a reset, never a read timeout
/// (which would mean it left the client hanging).
fn assert_closed(read: &std::io::Result<usize>) {
    let closed = match read {
        Ok(n) => *n == 0,
        Err(e) => matches!(
            e.kind(),
            ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted | ErrorKind::BrokenPipe
        ),
    };
    assert!(closed, "expected a closed connection, got {read:?}");
}
