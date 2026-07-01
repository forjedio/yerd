//! Reading the cloudflared origin cert (`cert.pem`) and resolving the authorized
//! Cloudflare zone (domain) from it.
//!
//! `cloudflared tunnel login` saves a `cert.pem` whose `ARGO TUNNEL TOKEN` block
//! is base64-encoded JSON `{zoneID, accountID, apiToken}` - it carries no human
//! domain name, so the zone name is resolved with one Cloudflare API call
//! (`GET /zones/{id}`) using the embedded token.

use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use serde::Deserialize;
use yerd_platform::PlatformDirs;

/// Bound on the zone-resolution API call so a `list` never stalls.
const ZONE_TIMEOUT: Duration = Duration::from_secs(5);

/// Process-wide cache of the resolved zone, keyed by the cert's modification
/// time. The zone changes only when the cert does (a fresh login), so this lets
/// `list()` answer from memory instead of hitting the Cloudflare API on every
/// poll. A changed mtime (re-login) misses and re-resolves.
static ZONE_CACHE: Mutex<Option<(SystemTime, String)>> = Mutex::new(None);

/// The fields read from the cert's `ARGO TUNNEL TOKEN` block. No `Debug` derive:
/// `api_token` is a live Cloudflare credential and must never be formatted into
/// a log or error.
#[derive(Deserialize)]
pub struct OriginToken {
    /// The authorized zone's id.
    #[serde(rename = "zoneID")]
    pub zone_id: String,
    /// The scoped API token used to query the zone.
    #[serde(rename = "apiToken")]
    pub api_token: String,
}

/// Decode the `ARGO TUNNEL TOKEN` (base64 JSON) from a cloudflared `cert.pem`.
#[must_use]
pub fn parse_origin_token(pem: &str) -> Option<OriginToken> {
    let body = extract_pem_block(pem, "ARGO TUNNEL TOKEN")?;
    let bytes = base64_decode(&body)?;
    serde_json::from_slice(&bytes).ok()
}

/// Resolve the authorized zone's domain name via `GET /zones/{id}`, memoized by
/// the cert's mtime so repeated `list()` calls don't re-hit the API. Returns
/// `None` if the cert is absent/unparseable or the API doesn't answer in time.
pub async fn resolve_zone(dirs: &PlatformDirs) -> Option<String> {
    let cert_path = super::named::origincert(dirs);
    let mtime = std::fs::metadata(&cert_path)
        .and_then(|m| m.modified())
        .ok()?;
    if let Some(zone) = cached_zone(mtime) {
        return Some(zone);
    }

    let pem = std::fs::read_to_string(&cert_path).ok()?;
    let token = parse_origin_token(&pem)?;
    if token.zone_id.is_empty() {
        return None;
    }
    let url = format!(
        "https://api.cloudflare.com/client/v4/zones/{}",
        token.zone_id
    );
    let client = reqwest::Client::builder().build().ok()?;
    let resp = client
        .get(&url)
        .bearer_auth(&token.api_token)
        .timeout(ZONE_TIMEOUT)
        .send()
        .await
        .ok()?;
    let bytes = resp.bytes().await.ok()?;
    let parsed: ZoneResp = serde_json::from_slice(&bytes).ok()?;
    let zone = parsed.result.map(|z| z.name)?;
    if let Ok(mut guard) = ZONE_CACHE.lock() {
        *guard = Some((mtime, zone.clone()));
    }
    Some(zone)
}

/// The cached zone if it was resolved from a cert with this exact mtime.
fn cached_zone(mtime: SystemTime) -> Option<String> {
    let guard = ZONE_CACHE.lock().ok()?;
    guard
        .as_ref()
        .filter(|(t, _)| *t == mtime)
        .map(|(_, zone)| zone.clone())
}

/// One zone object from the Cloudflare API `result`.
#[derive(Deserialize)]
struct Zone {
    name: String,
}

/// The Cloudflare `GET /zones/{id}` envelope (the subset we read).
#[derive(Deserialize)]
struct ZoneResp {
    result: Option<Zone>,
}

/// Pull the joined base64 body of a named PEM block (`-----BEGIN <name>-----`).
fn extract_pem_block(pem: &str, name: &str) -> Option<String> {
    let begin = format!("-----BEGIN {name}-----");
    let end = format!("-----END {name}-----");
    let mut body = String::new();
    let mut inside = false;
    for line in pem.lines() {
        let line = line.trim();
        if line == begin {
            inside = true;
        } else if line == end {
            return Some(body);
        } else if inside {
            body.push_str(line);
        }
    }
    None
}

/// Lenient RFC 4648 base64 decode (standard alphabet; padding and whitespace are
/// skipped). Returns `None` on an invalid character; a non-multiple-of-4 length
/// drops the trailing partial group rather than erroring (fine for the trusted
/// cloudflared cert). Hand-rolled (~25 lines) so this one small need doesn't add
/// a direct `base64` dependency.
fn base64_decode(s: &str) -> Option<Vec<u8>> {
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
            assert_eq!(base64_decode(&b64(s.as_bytes())).unwrap(), s.as_bytes());
        }
    }

    #[test]
    fn base64_tolerates_whitespace_and_rejects_junk() {
        assert_eq!(base64_decode("aGVs\nbG8=").unwrap(), b"hello");
        assert!(base64_decode("not*valid").is_none());
    }

    #[test]
    fn parses_token_from_a_cert() {
        let json = r#"{"zoneID":"abc123","accountID":"acc","apiToken":"tok"}"#;
        let pem = format!(
            "-----BEGIN ARGO TUNNEL TOKEN-----\n{}\n-----END ARGO TUNNEL TOKEN-----\n",
            b64(json.as_bytes())
        );
        let token = parse_origin_token(&pem).unwrap();
        assert_eq!(token.zone_id, "abc123");
        assert_eq!(token.api_token, "tok");
    }

    #[test]
    fn missing_block_yields_none() {
        assert!(
            parse_origin_token("-----BEGIN CERTIFICATE-----\nzz\n-----END CERTIFICATE-----")
                .is_none()
        );
        assert!(parse_origin_token("not a pem").is_none());
    }
}
