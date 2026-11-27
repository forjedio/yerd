//! Reading a token from stdin for `yerd tunnel ngrok token` and
//! `yerd tunnel zrok enable` when it isn't given as an argument.
//!
//! A terminal gets a prompt on stderr and the line is read with echo turned
//! off, so the token never shows on screen; the terminal's settings are put
//! back whether or not the read succeeds. Piped stdin is read as a plain line.

use std::io::BufRead;

use crate::error::ClientError;

/// Read one secret line from stdin. The value is trimmed; EOF or an empty line
/// is a usage error.
///
/// # Errors
///
/// [`ClientError::Usage`] when stdin can't be read or holds no token.
pub fn read_secret_line(prompt: &str) -> Result<String, ClientError> {
    use std::io::{IsTerminal as _, Write as _};
    let stdin = std::io::stdin();
    let line = if stdin.is_terminal() {
        eprint!("{prompt}");
        let _ = std::io::stderr().flush();
        read_terminal_line(&stdin)
    } else {
        read_line(&mut stdin.lock())
    };
    let line =
        line.map_err(|e| ClientError::Usage(format!("could not read the token from stdin: {e}")))?;
    secret_from_line(&line)
}

/// The token in one line read from stdin.
///
/// # Errors
///
/// [`ClientError::Usage`] when the line is empty after trimming.
pub fn secret_from_line(line: &str) -> Result<String, ClientError> {
    let token = line.trim();
    if token.is_empty() {
        return Err(ClientError::Usage(
            "no token given - pass it as an argument or on stdin".to_owned(),
        ));
    }
    Ok(token.to_owned())
}

fn read_line(reader: &mut impl BufRead) -> std::io::Result<String> {
    let mut line = String::new();
    reader.read_line(&mut line)?;
    Ok(line)
}

#[cfg(unix)]
fn read_terminal_line(stdin: &std::io::Stdin) -> std::io::Result<String> {
    use std::os::fd::AsFd as _;
    let _echo_off = EchoOff::new(stdin.as_fd())?;
    read_line(&mut stdin.lock())
}

#[cfg(not(unix))]
fn read_terminal_line(stdin: &std::io::Stdin) -> std::io::Result<String> {
    read_line(&mut stdin.lock())
}

/// Turns a terminal's echo off for as long as it lives and restores the
/// original settings on drop, so an early return or panic can't leave the
/// terminal silent. `ECHONL` keeps the newline visible after Enter.
#[cfg(unix)]
struct EchoOff<'fd> {
    fd: std::os::fd::BorrowedFd<'fd>,
    original: nix::sys::termios::Termios,
}

#[cfg(unix)]
impl<'fd> EchoOff<'fd> {
    fn new(fd: std::os::fd::BorrowedFd<'fd>) -> std::io::Result<Self> {
        use nix::sys::termios::{tcgetattr, tcsetattr, LocalFlags, SetArg};
        let original = tcgetattr(fd)?;
        let mut silent = original.clone();
        silent.local_flags.remove(LocalFlags::ECHO);
        silent.local_flags.insert(LocalFlags::ECHONL);
        tcsetattr(fd, SetArg::TCSANOW, &silent)?;
        Ok(Self { fd, original })
    }
}

#[cfg(unix)]
impl Drop for EchoOff<'_> {
    fn drop(&mut self) {
        use nix::sys::termios::{tcsetattr, SetArg};
        let _ = tcsetattr(self.fd, SetArg::TCSANOW, &self.original);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn secret_from_line_trims_and_rejects_empty() {
        for (line, want) in [
            ("tok_123\n", Some("tok_123")),
            ("  tok_123 \r\n", Some("tok_123")),
            ("\n", None),
            ("", None),
            ("   \n", None),
        ] {
            assert_eq!(secret_from_line(line).ok().as_deref(), want, "{line:?}");
        }
    }

    #[test]
    fn read_line_reads_one_line_from_a_pipe() {
        let mut input = std::io::Cursor::new("first\nsecond\n");
        assert_eq!(read_line(&mut input).unwrap(), "first\n");
    }

    #[cfg(unix)]
    #[test]
    fn echo_off_silences_the_terminal_and_restores_it_on_drop() {
        use nix::sys::termios::{tcgetattr, LocalFlags};
        use std::os::fd::AsFd as _;
        let pty = nix::pty::openpty(None, None).unwrap();
        let fd = pty.slave.as_fd();
        let before = tcgetattr(fd).unwrap().local_flags;
        assert!(before.contains(LocalFlags::ECHO));
        {
            let _guard = EchoOff::new(fd).unwrap();
            let during = tcgetattr(fd).unwrap().local_flags;
            assert!(!during.contains(LocalFlags::ECHO));
            assert!(during.contains(LocalFlags::ECHONL));
        }
        assert_eq!(tcgetattr(fd).unwrap().local_flags, before);
    }

    #[cfg(unix)]
    #[test]
    fn echo_off_fails_cleanly_on_a_non_terminal() {
        use std::os::fd::AsFd as _;
        let file = tempfile::tempfile().unwrap();
        assert!(EchoOff::new(file.as_fd()).is_err());
    }
}
