//! Incremental CGI response-head parsing.
//!
//! PHP-FPM delivers a response as a stream of STDOUT records whose leading
//! bytes are a CGI header block ending at a blank line. The forwarder feeds
//! those records here as they arrive and starts streaming the body the moment
//! [`HeadFeed::Complete`] comes back, so nothing waits for `END_REQUEST`.
//!
//! Buffering until the terminator is found is what makes a record boundary
//! falling inside a `\r\n\r\n` harmless: a feed resumes its scan three bytes
//! back into what was already there, so the split point never matters.

use http::StatusCode;

/// Cap on the buffered header block. Generous next to nginx's default
/// `fastcgi_buffer_size` of 4-8 KiB: a head past this is a runaway backend,
/// not a real response.
const MAX_HEAD_BYTES: usize = 64 * 1024;

/// Bytes of the previous buffer a rescan has to revisit so a terminator split
/// across two feeds is still found: one less than the longest terminator.
const TERMINATOR_OVERLAP: usize = 3;

/// A parsed CGI response head, plus any body bytes that arrived with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    /// Code from the CGI `Status:` line, or 200 when there wasn't one.
    pub status: StatusCode,
    /// Header lines in wire order, with `Status:` removed.
    pub headers: Vec<(String, String)>,
    /// Body bytes that followed the terminator in the same feed.
    pub body_remainder: Vec<u8>,
}

/// What feeding a STDOUT chunk to a [`HeadAccumulator`] produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeadFeed {
    /// No terminator yet; keep feeding.
    Pending,
    /// The header block is complete.
    Complete(Head),
    /// The block passed the size cap with no terminator in sight.
    TooLarge,
}

/// Buffers STDOUT bytes until the CGI header block terminates.
#[derive(Debug, Default)]
pub struct HeadAccumulator {
    buf: Vec<u8>,
}

impl HeadAccumulator {
    /// A new, empty accumulator.
    #[must_use]
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    /// Add `chunk` and rescan for the header terminator.
    pub fn feed(&mut self, chunk: &[u8]) -> HeadFeed {
        let scan_from = self.buf.len().saturating_sub(TERMINATOR_OVERLAP);
        self.buf.extend_from_slice(chunk);
        match find_header_terminator(&self.buf, scan_from) {
            Some((offset, terminator_len)) => {
                let (head, rest) = self.buf.split_at(offset);
                let body_remainder = rest.get(terminator_len..).unwrap_or_default().to_vec();
                let (status, headers) = parse_head(head);
                HeadFeed::Complete(Head {
                    status,
                    headers,
                    body_remainder,
                })
            }
            None if self.buf.len() > MAX_HEAD_BYTES => HeadFeed::TooLarge,
            None => HeadFeed::Pending,
        }
    }

    /// The backend finished the response without ever terminating the header
    /// block. Everything buffered is the head and the body is empty, which is
    /// what the pre-streaming forwarder did with the same input.
    #[must_use]
    pub fn finish(self) -> Head {
        let (status, headers) = parse_head(&self.buf);
        Head {
            status,
            headers,
            body_remainder: Vec::new(),
        }
    }
}

/// Parse a CGI header block: `Status: NNN Reason` drives the status code and is
/// not surfaced as a header; everything else is passed through trimmed. Lines
/// without a colon are skipped.
///
/// Decoding is lossy and per line, so a header carrying bytes that aren't UTF-8
/// (a `setrawcookie` payload, a latin-1 filename in `Content-Disposition`) costs
/// only that one value. Decoding the block as a whole would instead drop every
/// header and the `Status:` line with them, turning a 302 into a bare 200 with
/// no `Location`.
fn parse_head(head: &[u8]) -> (StatusCode, Vec<(String, String)>) {
    let mut status = StatusCode::OK;
    let mut headers: Vec<(String, String)> = Vec::new();
    for raw_line in head.split(|byte| *byte == b'\n') {
        let decoded = String::from_utf8_lossy(raw_line);
        let line = decoded.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim();
        let value = value.trim();
        if name.eq_ignore_ascii_case("Status") {
            if let Some(code) = parse_cgi_status(value) {
                status = code;
            }
        } else {
            headers.push((name.to_owned(), value.to_owned()));
        }
    }
    (status, headers)
}

/// Parse a CGI `Status:` value - `"200 OK"` or a bare `"200"` - into a status
/// code. `None` when it isn't a valid code, leaving the caller's default.
fn parse_cgi_status(value: &str) -> Option<StatusCode> {
    let code = value.split_once(' ').map_or(value, |(code, _)| code);
    StatusCode::from_u16(code.parse::<u16>().ok()?).ok()
}

/// Locate the blank line ending the header block, returning
/// `(offset, terminator_len)`. `None` when the buffer holds no terminator yet.
///
/// Scanning starts at `from`, which lets a repeated feed skip the bytes it has
/// already rejected instead of rescanning the whole buffer each time.
fn find_header_terminator(buf: &[u8], from: usize) -> Option<(usize, usize)> {
    for i in from..buf.len() {
        if buf.get(i..i + 4) == Some(b"\r\n\r\n".as_slice()) {
            return Some((i, 4));
        }
        if buf.get(i..i + 2) == Some(b"\n\n".as_slice()) {
            return Some((i, 2));
        }
    }
    None
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

    fn complete(feed: HeadFeed) -> Head {
        match feed {
            HeadFeed::Complete(head) => head,
            other => panic!("expected Complete, got {other:?}"),
        }
    }

    fn feed_all(chunks: &[&[u8]]) -> HeadFeed {
        let mut acc = HeadAccumulator::new();
        let mut last = HeadFeed::Pending;
        for chunk in chunks {
            last = acc.feed(chunk);
            if matches!(last, HeadFeed::Complete(_) | HeadFeed::TooLarge) {
                break;
            }
        }
        last
    }

    #[test]
    fn whole_head_and_body_in_one_feed() {
        let cases: [(&[u8], &str); 2] = [
            (b"Content-Type: text/plain\r\n\r\nhello", "text/plain"),
            (b"Content-Type: text/html\n\nhello", "text/html"),
        ];
        for (input, content_type) in cases {
            let head = complete(feed_all(&[input]));
            assert_eq!(head.status, StatusCode::OK);
            assert_eq!(
                head.headers,
                vec![("Content-Type".to_owned(), content_type.to_owned())]
            );
            assert_eq!(head.body_remainder, b"hello");
        }
    }

    #[test]
    fn head_split_across_feeds_completes_on_the_later_chunk() {
        let mut acc = HeadAccumulator::new();
        assert_eq!(acc.feed(b"Content-Type: text/plain\r\n"), HeadFeed::Pending);
        assert_eq!(acc.feed(b"X-Extra: 1\r\n"), HeadFeed::Pending);
        let head = complete(acc.feed(b"\r\nbody bytes"));
        assert_eq!(
            head.headers,
            vec![
                ("Content-Type".to_owned(), "text/plain".to_owned()),
                ("X-Extra".to_owned(), "1".to_owned()),
            ]
        );
        assert_eq!(head.body_remainder, b"body bytes");
    }

    #[test]
    fn terminator_straddling_a_feed_boundary_at_every_split_point() {
        let whole: &[u8] = b"Content-Type: text/plain\r\n\r\nchunk";
        let terminator_start = 24;
        for split in 1..4 {
            let at = terminator_start + split;
            let head = complete(feed_all(&[&whole[..at], &whole[at..]]));
            assert_eq!(head.status, StatusCode::OK);
            assert_eq!(
                head.headers,
                vec![("Content-Type".to_owned(), "text/plain".to_owned())],
                "split at {at}"
            );
            assert_eq!(head.body_remainder, b"chunk", "split at {at}");
        }
    }

    #[test]
    fn status_line_drives_the_code_and_is_not_surfaced() {
        let cases: [(&[u8], StatusCode); 4] = [
            (b"Status: 404 Not Found\r\n\r\n", StatusCode::NOT_FOUND),
            (b"Status: 301\r\n\r\n", StatusCode::MOVED_PERMANENTLY),
            (b"Status: abc\r\n\r\n", StatusCode::OK),
            (b"Status: 99\r\n\r\n", StatusCode::OK),
        ];
        for (input, expected) in cases {
            let head = complete(feed_all(&[input]));
            assert_eq!(head.status, expected);
            assert!(head
                .headers
                .iter()
                .all(|(name, _)| !name.eq_ignore_ascii_case("Status")));
        }
    }

    #[test]
    fn status_alongside_other_headers_keeps_them() {
        let head = complete(feed_all(&[b"Status: 301 Moved\r\nLocation: /x\r\n\r\n"]));
        assert_eq!(head.status, StatusCode::MOVED_PERMANENTLY);
        assert_eq!(head.headers, vec![("Location".to_owned(), "/x".to_owned())]);
        assert!(head.body_remainder.is_empty());
    }

    #[test]
    fn oversize_head_without_a_terminator_is_too_large() {
        let mut acc = HeadAccumulator::new();
        let filler = vec![b'x'; 8 * 1024];
        let mut outcome = HeadFeed::Pending;
        for _ in 0..9 {
            outcome = acc.feed(&filler);
        }
        assert_eq!(outcome, HeadFeed::TooLarge);
    }

    #[test]
    fn head_at_the_cap_still_completes_when_terminated() {
        let mut acc = HeadAccumulator::new();
        let mut line = b"X-Pad: ".to_vec();
        line.extend(std::iter::repeat_n(b'y', 1_000));
        line.extend_from_slice(b"\r\n");
        for _ in 0..8 {
            assert_eq!(acc.feed(&line), HeadFeed::Pending);
        }
        let head = complete(acc.feed(b"\r\ntail"));
        assert_eq!(head.headers.len(), 8);
        assert_eq!(head.body_remainder, b"tail");
    }

    #[test]
    fn finish_treats_an_unterminated_block_as_the_whole_head() {
        let mut acc = HeadAccumulator::new();
        assert_eq!(acc.feed(b"Content-Type: text/plain"), HeadFeed::Pending);
        let head = acc.finish();
        assert_eq!(head.status, StatusCode::OK);
        assert_eq!(
            head.headers,
            vec![("Content-Type".to_owned(), "text/plain".to_owned())]
        );
        assert!(head.body_remainder.is_empty());
    }

    #[test]
    fn finish_on_an_empty_accumulator_is_a_bare_200() {
        let head = HeadAccumulator::new().finish();
        assert_eq!(head.status, StatusCode::OK);
        assert!(head.headers.is_empty());
        assert!(head.body_remainder.is_empty());
    }

    #[test]
    fn body_remainder_is_empty_when_nothing_trails_the_terminator() {
        let head = complete(feed_all(&[b"Content-Type: text/plain\r\n\r\n"]));
        assert!(head.body_remainder.is_empty());
    }

    #[test]
    fn crlf_terminator_wins_over_a_later_bare_lf_pair() {
        let head = complete(feed_all(&[b"A: b\r\n\r\nbody\n\nmore"]));
        assert_eq!(head.headers, vec![("A".to_owned(), "b".to_owned())]);
        assert_eq!(head.body_remainder, b"body\n\nmore");
    }

    #[test]
    fn lines_without_a_colon_are_skipped() {
        let head = complete(feed_all(&[b"garbage line\r\nA: b\r\n\r\n"]));
        assert_eq!(head.headers, vec![("A".to_owned(), "b".to_owned())]);
    }

    #[test]
    fn non_utf8_value_is_lossy_and_costs_only_its_own_line() {
        let head = complete(feed_all(&[
            b"Status: 302 Found\r\nSet-Cookie: \xff\xfe\r\nLocation: /next\r\n\r\nbody",
        ]));
        assert_eq!(head.status, StatusCode::FOUND);
        assert_eq!(
            head.headers,
            vec![
                ("Set-Cookie".to_owned(), "\u{fffd}\u{fffd}".to_owned()),
                ("Location".to_owned(), "/next".to_owned()),
            ]
        );
        assert_eq!(head.body_remainder, b"body");
    }

    #[test]
    fn many_tiny_feeds_still_find_a_terminator_the_scan_resumes_past() {
        let mut acc = HeadAccumulator::new();
        let whole: &[u8] = b"Content-Type: text/plain\r\n\r\ntail";
        let mut outcome = HeadFeed::Pending;
        for byte in whole {
            outcome = acc.feed(std::slice::from_ref(byte));
            if matches!(outcome, HeadFeed::Complete(_)) {
                break;
            }
        }
        let head = complete(outcome);
        assert_eq!(
            head.headers,
            vec![("Content-Type".to_owned(), "text/plain".to_owned())]
        );
        assert!(head.body_remainder.is_empty());
    }
}
