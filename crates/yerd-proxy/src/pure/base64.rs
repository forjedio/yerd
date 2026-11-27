//! A small, lenient RFC 4648 base64 decoder shared by the tunnel basic-auth
//! check and the daemon's cloudflared origin-cert reader.

/// Lenient RFC 4648 base64 decode (standard alphabet; padding and whitespace are
/// skipped). Returns `None` on an invalid character; a non-multiple-of-4 length
/// drops the trailing partial group rather than erroring. Hand-rolled (~25
/// lines) so this one small need doesn't add a direct `base64` dependency.
#[must_use]
pub fn decode(s: &str) -> Option<Vec<u8>> {
    let sextet = |c: u8| -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some(u32::from(c - b'A')),
            b'a'..=b'z' => Some(u32::from(c - b'a') + 26),
            b'0'..=b'9' => Some(u32::from(c - b'0') + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    };
    let mut out = Vec::new();
    let mut acc = 0u32;
    let mut bits = 0u32;
    for &c in s.as_bytes() {
        if c == b'=' || c.is_ascii_whitespace() {
            continue;
        }
        acc = (acc << 6) | sextet(c)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((acc >> bits) & 0xFF).ok()?);
        }
    }
    Some(out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn b64(data: &[u8]) -> String {
        const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in data.chunks(3) {
            let b0 = u32::from(chunk[0]);
            let b1 = u32::from(chunk.get(1).copied().unwrap_or(0));
            let b2 = u32::from(chunk.get(2).copied().unwrap_or(0));
            let n = (b0 << 16) | (b1 << 8) | b2;
            out.push(A[(n >> 18 & 63) as usize] as char);
            out.push(A[(n >> 12 & 63) as usize] as char);
            out.push(if chunk.len() > 1 {
                A[(n >> 6 & 63) as usize] as char
            } else {
                '='
            });
            out.push(if chunk.len() > 2 {
                A[(n & 63) as usize] as char
            } else {
                '='
            });
        }
        out
    }

    #[test]
    fn base64_round_trips() {
        for s in [
            "",
            "f",
            "fo",
            "foo",
            "foob",
            "fooba",
            "foobar",
            "hello cloudflared",
        ] {
            assert_eq!(decode(&b64(s.as_bytes())).unwrap(), s.as_bytes());
        }
    }

    #[test]
    fn base64_tolerates_whitespace_and_rejects_junk() {
        assert_eq!(decode("aGVs\nbG8=").unwrap(), b"hello");
        assert!(decode("not*valid").is_none());
    }
}
