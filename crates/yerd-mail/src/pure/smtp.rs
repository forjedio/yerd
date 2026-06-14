//! A minimal, pure SMTP receiver state machine.
//!
//! It speaks just enough of RFC 5321 to capture mail from a local app's mailer:
//! `EHLO`/`HELO`, `MAIL FROM`, `RCPT TO`, `DATA`, `RSET`, `NOOP`, `QUIT`. There
//! is no AUTH, no TLS, and no relaying — every recipient is accepted and the
//! message body is captured verbatim.
//!
//! This module owns no sockets. The I/O layer ([`crate::io::server`]) reads a
//! line, calls [`Session::command`], and acts on the returned [`Reply`]; in
//! `DATA` mode it accumulates raw bytes and finishes with [`Session::finish_data`].

/// One captured message: the SMTP envelope plus the verbatim, dot-unstuffed
/// `DATA` payload (an RFC 5322 message ready for MIME parsing).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawMessage {
    /// The `MAIL FROM` address (between the angle brackets), if any.
    pub envelope_from: String,
    /// The `RCPT TO` addresses, in the order given.
    pub recipients: Vec<String>,
    /// The dot-unstuffed message bytes.
    pub raw: Vec<u8>,
}

/// What the I/O layer should do after a command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// Write this reply (already CRLF-terminated) and keep reading commands.
    Line(String),
    /// Write this reply, then switch to collecting `DATA` until `\r\n.\r\n`.
    StartData(String),
    /// Write this reply, then close the connection (`QUIT`).
    Close(String),
}

/// In-progress SMTP session state (envelope being built up).
#[derive(Debug, Default)]
pub struct Session {
    from: Option<String>,
    recipients: Vec<String>,
}

impl Session {
    /// A fresh session.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The greeting to send immediately on connect.
    #[must_use]
    pub fn greeting() -> &'static str {
        "220 yerd mail capture ready\r\n"
    }

    /// Handle one command line (without the trailing CRLF). Returns the reply to
    /// send and whether to enter `DATA` mode or close.
    // `NOOP` and the lenient catch-all intentionally share a reply; keeping them
    // as separate arms documents the protocol surface.
    #[allow(clippy::match_same_arms)]
    pub fn command(&mut self, line: &str) -> Reply {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        let verb = trimmed
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_ascii_uppercase();
        match verb.as_str() {
            "HELO" | "EHLO" => Reply::Line("250 yerd\r\n".to_string()),
            "MAIL" => {
                // `MAIL FROM` begins a new transaction (RFC 5321 §4.1.1.2): clear
                // any recipients left over from an abandoned prior envelope so they
                // can't leak into this message's captured metadata.
                self.recipients.clear();
                self.from = Some(extract_address(trimmed));
                Reply::Line("250 OK\r\n".to_string())
            }
            "RCPT" => {
                self.recipients.push(extract_address(trimmed));
                Reply::Line("250 OK\r\n".to_string())
            }
            "DATA" => {
                if self.recipients.is_empty() {
                    Reply::Line("503 RCPT first\r\n".to_string())
                } else {
                    Reply::StartData("354 End data with <CR><LF>.<CR><LF>\r\n".to_string())
                }
            }
            "RSET" => {
                self.from = None;
                self.recipients.clear();
                Reply::Line("250 OK\r\n".to_string())
            }
            "NOOP" => Reply::Line("250 OK\r\n".to_string()),
            "QUIT" => Reply::Close("221 Bye\r\n".to_string()),
            // Be lenient: a capture sink accepts whatever a dev mailer sends.
            "" => Reply::Line("500 Syntax error\r\n".to_string()),
            _ => Reply::Line("250 OK\r\n".to_string()),
        }
    }

    /// Consume the raw `DATA` bytes (everything between the `354` and the
    /// terminating `\r\n.\r\n`, with that terminator already stripped by the I/O
    /// layer) and produce the captured message. Resets the envelope so the same
    /// connection may send another message.
    pub fn finish_data(&mut self, data: &[u8]) -> RawMessage {
        RawMessage {
            envelope_from: self.from.take().unwrap_or_default(),
            recipients: std::mem::take(&mut self.recipients),
            raw: unstuff(data),
        }
    }
}

/// Undo SMTP dot-stuffing: a line that began with `.` was sent with an extra
/// leading `.` (RFC 5321 §4.5.2). Strip one leading `.` from any line that has
/// two or more. Operates on the raw `\r\n`-delimited payload.
#[must_use]
pub fn unstuff(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut at_line_start = true;
    for &b in data {
        if at_line_start && b == b'.' {
            // Drop exactly one leading dot of a stuffed line; keep the rest.
            at_line_start = false;
            continue;
        }
        out.push(b);
        at_line_start = b == b'\n';
    }
    out
}

/// Extract the address from a `MAIL FROM:<addr>` / `RCPT TO:<addr>` line. Falls
/// back to the text after the first `:` (trimmed) when there are no brackets.
fn extract_address(line: &str) -> String {
    if let (Some(start), Some(end)) = (line.find('<'), line.rfind('>')) {
        if let Some(inner) = line.get(start + 1..end) {
            return inner.trim().to_string();
        }
    }
    match line.split_once(':') {
        Some((_, rest)) => rest.trim().to_string(),
        None => String::new(),
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

    #[test]
    fn greeting_is_220() {
        assert!(Session::greeting().starts_with("220 "));
    }

    #[test]
    fn full_session_captures_envelope_and_body() {
        let mut s = Session::new();
        assert_eq!(
            s.command("EHLO localhost"),
            Reply::Line("250 yerd\r\n".into())
        );
        assert_eq!(
            s.command("MAIL FROM:<hello@example.com>"),
            Reply::Line("250 OK\r\n".into())
        );
        assert_eq!(
            s.command("RCPT TO:<test@test.com>"),
            Reply::Line("250 OK\r\n".into())
        );
        match s.command("DATA") {
            Reply::StartData(r) => assert!(r.starts_with("354 ")),
            other => panic!("expected StartData, got {other:?}"),
        }
        let msg = s.finish_data(b"Subject: Hi\r\n\r\nBody\r\n");
        assert_eq!(msg.envelope_from, "hello@example.com");
        assert_eq!(msg.recipients, vec!["test@test.com".to_string()]);
        assert_eq!(msg.raw, b"Subject: Hi\r\n\r\nBody\r\n");
    }

    #[test]
    fn data_requires_recipient() {
        let mut s = Session::new();
        assert_eq!(s.command("DATA"), Reply::Line("503 RCPT first\r\n".into()));
    }

    #[test]
    fn quit_closes() {
        let mut s = Session::new();
        match s.command("QUIT") {
            Reply::Close(r) => assert!(r.starts_with("221 ")),
            other => panic!("expected Close, got {other:?}"),
        }
    }

    #[test]
    fn rset_clears_envelope() {
        let mut s = Session::new();
        s.command("MAIL FROM:<a@b.c>");
        s.command("RCPT TO:<d@e.f>");
        s.command("RSET");
        s.command("RCPT TO:<g@h.i>");
        let msg = s.finish_data(b"x");
        assert_eq!(msg.envelope_from, "");
        assert_eq!(msg.recipients, vec!["g@h.i".to_string()]);
    }

    #[test]
    fn second_mail_from_resets_recipients() {
        // A new MAIL FROM before DATA must drop the prior envelope's recipients.
        let mut s = Session::new();
        s.command("MAIL FROM:<a@b.c>");
        s.command("RCPT TO:<stale@old.test>");
        s.command("MAIL FROM:<b@c.d>");
        s.command("RCPT TO:<fresh@new.test>");
        let msg = s.finish_data(b"x");
        assert_eq!(msg.envelope_from, "b@c.d");
        assert_eq!(msg.recipients, vec!["fresh@new.test".to_string()]);
    }

    #[test]
    fn unstuff_removes_one_leading_dot() {
        // A body line ".hidden" is wire-encoded as "..hidden".
        assert_eq!(unstuff(b"a\r\n..hidden\r\n"), b"a\r\n.hidden\r\n");
        // A genuine single dot at line start (rare) is also de-stuffed by one.
        assert_eq!(unstuff(b".x\r\n"), b"x\r\n");
        // No dots → unchanged.
        assert_eq!(unstuff(b"hello\r\nworld\r\n"), b"hello\r\nworld\r\n");
    }

    #[test]
    fn extract_handles_missing_brackets() {
        let mut s = Session::new();
        s.command("MAIL FROM: bare@example.com");
        s.command("RCPT TO:<x@y.z>");
        let msg = s.finish_data(b"x");
        assert_eq!(msg.envelope_from, "bare@example.com");
    }

    #[test]
    fn unknown_command_is_accepted_leniently() {
        let mut s = Session::new();
        assert_eq!(s.command("XYZZY"), Reply::Line("250 OK\r\n".into()));
    }
}
