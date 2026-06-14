//! Decode a captured `.eml` into the owned wire types ([`MailSummary`],
//! [`MailDetail`]).
//!
//! `mail-parser` is a zero-copy parser (its output borrows the input via `Cow`
//! and lifetimes), so nothing it returns can cross the IPC wire or be stored
//! directly. Every field is cloned out into an owned `String`/`u64` here. The
//! HTML body additionally has `cid:` references rewritten to inline `data:`
//! URLs so a sandboxed viewer can render embedded images without network access.

use mail_parser::{Addr, Address, Message, MessageParser, MessagePart, MimeHeaders};
use yerd_ipc::{MailDetail, MailHeader, MailSummary};

/// Decode just the metadata of a captured message.
#[must_use]
pub fn summary(id: &str, raw: &[u8]) -> MailSummary {
    let msg = MessageParser::default().parse(raw).unwrap_or_default();
    MailSummary {
        id: id.to_string(),
        from: render_from(&msg),
        to: render_to(&msg),
        subject: msg.subject().unwrap_or_default().to_string(),
        date_epoch: date_epoch(&msg),
    }
}

/// Decode the full content of a captured message (headers + decoded bodies).
#[must_use]
pub fn detail(id: &str, raw: &[u8]) -> MailDetail {
    let msg = MessageParser::default().parse(raw).unwrap_or_default();

    let headers = msg
        .headers()
        .iter()
        .map(|h| MailHeader {
            name: h.name().to_string(),
            value: raw
                .get(h.offset_start as usize..h.offset_end as usize)
                .map(|b| String::from_utf8_lossy(b).trim().to_string())
                .unwrap_or_default(),
        })
        .collect();

    let text_body = msg.body_text(0).map(std::borrow::Cow::into_owned);
    // `body_html` synthesises HTML from a text-only message; only surface a real
    // HTML body when the message genuinely carries a `text/html` part, so the
    // viewer can fall back to `text_body` otherwise.
    let html_body = if msg.parts.iter().any(is_html_part) {
        msg.body_html(0).map(|c| rewrite_cids(&msg, c.into_owned()))
    } else {
        None
    };

    MailDetail {
        id: id.to_string(),
        from: render_from(&msg),
        to: render_to(&msg),
        subject: msg.subject().unwrap_or_default().to_string(),
        date_epoch: date_epoch(&msg),
        headers,
        html_body,
        text_body,
    }
}

/// Whether a part is a genuine `text/html` body (not a text part we'd synthesise
/// HTML from).
fn is_html_part(part: &MessagePart) -> bool {
    part.content_type()
        .and_then(mail_parser::ContentType::subtype)
        .is_some_and(|s| s.eq_ignore_ascii_case("html"))
}

fn date_epoch(msg: &Message) -> u64 {
    msg.date()
        .map(mail_parser::DateTime::to_timestamp)
        .and_then(|t| u64::try_from(t).ok())
        .unwrap_or(0)
}

fn render_from(msg: &Message) -> String {
    render_addresses(msg.from())
        .into_iter()
        .next()
        .unwrap_or_default()
}

fn render_to(msg: &Message) -> Vec<String> {
    render_addresses(msg.to())
}

/// Render an [`Address`] (which may be a flat list or contain groups) into one
/// display string per mailbox.
fn render_addresses(addr: Option<&Address>) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(a) = addr {
        for mailbox in a.iter() {
            let s = render_addr(mailbox);
            if !s.is_empty() {
                out.push(s);
            }
        }
    }
    out
}

fn render_addr(addr: &Addr) -> String {
    match (addr.name(), addr.address()) {
        (Some(name), Some(email)) => format!("{name} <{email}>"),
        (None, Some(email)) => email.to_string(),
        (Some(name), None) => name.to_string(),
        (None, None) => String::new(),
    }
}

/// Replace `cid:` references in an HTML body with inline `data:` URLs built from
/// the message's inline attachments, so the body renders without network access.
/// Parts without a content-id (or the absence of any) leave the HTML unchanged.
fn rewrite_cids(msg: &Message, mut html: String) -> String {
    for part in msg.attachments() {
        let Some(cid) = part.content_id() else {
            continue;
        };
        let cid = cid.trim_matches(['<', '>']);
        if cid.is_empty() {
            continue;
        }
        let ctype = part.content_type().map_or_else(
            || "application/octet-stream".to_string(),
            |c| match c.subtype() {
                Some(sub) => format!("{}/{}", c.ctype(), sub),
                None => c.ctype().to_string(),
            },
        );
        let data_url = format!("data:{};base64,{}", ctype, base64_encode(part.contents()));
        html = html
            .replace(&format!("cid:{cid}"), &data_url)
            .replace(&format!("CID:{cid}"), &data_url);
    }
    html
}

/// Minimal standard-alphabet base64 encoder (no padding-free / URL variants).
/// Kept local to avoid pulling a base64 dependency for the one inline-image use.
fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    // Mask to 6 bits then look up; `unwrap_or` keeps it panic-free for clippy.
    let sextet = |v: u32| ALPHABET.get((v & 0x3f) as usize).copied().unwrap_or(b'A') as char;
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk.first().copied().unwrap_or(0);
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        let n = (u32::from(b0) << 16) | (u32::from(b1) << 8) | u32::from(b2);
        out.push(sextet(n >> 18));
        out.push(sextet(n >> 12));
        out.push(if chunk.len() > 1 { sextet(n >> 6) } else { '=' });
        out.push(if chunk.len() > 2 { sextet(n) } else { '=' });
    }
    out
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

    const SAMPLE: &[u8] = b"From: Example <hello@example.com>\r\n\
To: test@test.com\r\n\
Subject: Password Reset\r\n\
Date: Mon, 21 Jul 2025 21:07:30 +0000\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
\r\n\
Your OTP is 416063.\r\n";

    #[test]
    fn summary_extracts_envelope() {
        let s = summary("000001", SAMPLE);
        assert_eq!(s.id, "000001");
        assert_eq!(s.from, "Example <hello@example.com>");
        assert_eq!(s.to, vec!["test@test.com".to_string()]);
        assert_eq!(s.subject, "Password Reset");
        assert!(s.date_epoch > 0, "date should parse to an epoch");
    }

    #[test]
    fn detail_decodes_text_body_and_headers() {
        let d = detail("000001", SAMPLE);
        assert_eq!(d.subject, "Password Reset");
        assert!(d.text_body.as_deref().unwrap().contains("416063"));
        assert!(d.html_body.is_none());
        assert!(d
            .headers
            .iter()
            .any(|h| h.name.eq_ignore_ascii_case("subject")));
    }

    #[test]
    fn quoted_printable_html_is_decoded() {
        let raw = b"From: a@b.c\r\n\
To: d@e.f\r\n\
Subject: HTML\r\n\
Content-Type: text/html; charset=utf-8\r\n\
Content-Transfer-Encoding: quoted-printable\r\n\
\r\n\
<p>Hello =E2=9C=93 world</p>\r\n";
        let d = detail("000002", raw);
        let html = d.html_body.expect("html body");
        assert!(html.contains('\u{2713}'), "QP should decode: {html}");
    }

    #[test]
    fn base64_encode_matches_known_vector() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn cid_image_is_rewritten_to_data_url() {
        let raw = b"From: a@b.c\r\n\
To: d@e.f\r\n\
Subject: Inline\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/related; boundary=\"BB\"\r\n\
\r\n\
--BB\r\n\
Content-Type: text/html\r\n\
\r\n\
<img src=\"cid:img1\">\r\n\
--BB\r\n\
Content-Type: image/png\r\n\
Content-Transfer-Encoding: base64\r\n\
Content-ID: <img1>\r\n\
\r\n\
Zm9v\r\n\
--BB--\r\n";
        let d = detail("000003", raw);
        let html = d.html_body.expect("html body");
        assert!(
            html.contains("data:image/png;base64,"),
            "cid should become a data URL: {html}"
        );
        assert!(!html.contains("cid:img1"), "cid ref should be gone: {html}");
    }
}
