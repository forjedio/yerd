//! `yerd-forward`: launchd-spawned TCP forwarder for Yerd's macOS privileged
//! ports. See the library docs for the design.
//!
//! In launchd `Wait` mode stdin, stdout and stderr are the listening socket, so
//! nothing is printed once serving starts, and every write uses `writeln!` so a
//! failed write can never panic.

#![forbid(unsafe_code)]

use std::ffi::OsString;
use std::io::Write;
use std::process::ExitCode;

use yerd_forward::{parse_args, Command, Scope};

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    match parse_args(&args) {
        Ok(Command::Version) => {
            let _ = writeln!(
                std::io::stdout(),
                "yerd-forward {}",
                env!("CARGO_PKG_VERSION")
            );
            ExitCode::SUCCESS
        }
        Ok(Command::Forward { scope, to }) => run(scope, to),
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "yerd-forward: {e}");
            ExitCode::from(64)
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn run(_scope: Scope, _to: u16) -> ExitCode {
    let _ = writeln!(std::io::stderr(), "yerd-forward: not supported on this OS");
    ExitCode::from(78)
}

#[cfg(target_os = "macos")]
fn run(scope: Scope, to: u16) -> ExitCode {
    use std::net::TcpListener;
    use std::os::fd::AsFd;

    if nix::unistd::geteuid().is_root() {
        let _ = writeln!(std::io::stderr(), "yerd-forward: refusing to run as root");
        return ExitCode::from(77);
    }
    let Ok(fd) = std::io::stdin().as_fd().try_clone_to_owned() else {
        return ExitCode::from(66);
    };
    let listener = TcpListener::from(fd);
    if listener.local_addr().is_err() {
        let _ = writeln!(
            std::io::stderr(),
            "yerd-forward: stdin is not a listening socket (this binary is started by launchd)"
        );
        return ExitCode::from(66);
    }
    match yerd_forward::serve(listener, scope, to) {
        Ok(never) => match never {},
        Err(_) => ExitCode::from(74),
    }
}
