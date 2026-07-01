//! Pure scanners over `cloudflared` log output.
//!
//! `cloudflared` reports its assigned Quick Tunnel URL, named-tunnel edge
//! registration, and browser-login URL as lines of human-readable log text.
//! These hand-rolled scanners extract the bits the supervisor needs. They use no
//! regex dependency, no slice indexing, and never panic: an absent match is
//! simply `None`/`false`, so the supervisor stays in "starting" until the line
//! appears (or the readiness window elapses).

/// Find the assigned `https://<label>.trycloudflare.com` URL in a log chunk.
///
/// Returns the first match; `None` if the banner has not been printed yet.
#[must_use]
pub fn parse_quick_url(chunk: &str) -> Option<String> {
    split_tokens(chunk).find_map(extract_trycloudflare)
}

/// Find the browser-login auth URL (the first `https://…cloudflare.com…` token).
#[must_use]
pub fn find_auth_url(chunk: &str) -> Option<String> {
    split_tokens(chunk).find_map(|t| {
        let url = https_url_in(t)?;
        url.contains("cloudflare.com").then(|| url.to_owned())
    })
}

/// Whether the log output so far indicates a named tunnel finished registering
/// with the Cloudflare edge (i.e. it is now serving). Fed the whole log buffer,
/// not a single line.
#[must_use]
pub fn is_named_ready(chunk: &str) -> bool {
    const MARKERS: [&str; 2] = ["Registered tunnel connection", "Connection registered"];
    MARKERS.iter().any(|m| chunk.contains(m))
}

/// Extract the first UUID (`8-4-4-4-12` lowercase hex) from text, e.g. the id
/// `cloudflared tunnel create` prints ("Created tunnel NAME with id <uuid>").
#[must_use]
pub fn find_tunnel_id(text: &str) -> Option<String> {
    split_tokens(text)
        .map(|t| t.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-'))
        .find(|t| is_uuid(t))
        .map(str::to_owned)
}

/// Whether `s` is a canonical `8-4-4-4-12` hex UUID.
fn is_uuid(s: &str) -> bool {
    let groups = [8usize, 4, 4, 4, 12];
    let mut parts = s.split('-');
    for &len in &groups {
        match parts.next() {
            Some(p) if p.len() == len && p.bytes().all(|b| b.is_ascii_hexdigit()) => {}
            _ => return false,
        }
    }
    parts.next().is_none()
}

/// Split a chunk into candidate tokens on whitespace and the box-drawing /
/// quoting characters `cloudflared`'s banner wraps URLs in.
fn split_tokens(chunk: &str) -> impl Iterator<Item = &str> {
    chunk
        .split(|c: char| c.is_whitespace() || matches!(c, '|' | '"' | '\'' | '*' | '+' | '<' | '>'))
}

/// Extract a `https://` substring from a token, trimming trailing punctuation.
fn https_url_in(token: &str) -> Option<&str> {
    let start = token.find("https://")?;
    let url = token.get(start..)?;
    Some(url.trim_end_matches(['.', ',', ')', ']', '}', ';', ':']))
}

/// Return `https://<host>` when `token` holds a `*.trycloudflare.com` URL.
fn extract_trycloudflare(token: &str) -> Option<String> {
    let url = https_url_in(token)?;
    let host = url.strip_prefix("https://")?;
    let host = host.split('/').next().unwrap_or(host);
    if host.len() > ".trycloudflare.com".len() && host.ends_with(".trycloudflare.com") {
        Some(format!("https://{host}"))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_url_from_banner_box() {
        let chunk = "\
2024-01-01T00:00:00Z INF +--------------------------------------------------------+
2024-01-01T00:00:00Z INF |  Your quick Tunnel has been created! Visit it at:       |
2024-01-01T00:00:00Z INF |  https://calm-river-1234.trycloudflare.com              |
2024-01-01T00:00:00Z INF +--------------------------------------------------------+";
        assert_eq!(
            parse_quick_url(chunk).as_deref(),
            Some("https://calm-river-1234.trycloudflare.com")
        );
    }

    #[test]
    fn parses_url_on_a_plain_line() {
        let chunk = "registered tunnel https://abc-def.trycloudflare.com for you";
        assert_eq!(
            parse_quick_url(chunk).as_deref(),
            Some("https://abc-def.trycloudflare.com")
        );
    }

    #[test]
    fn ignores_noise_and_other_urls() {
        let chunk = "INF connecting to https://api.cloudflare.com/region edge=v2";
        assert_eq!(parse_quick_url(chunk), None);
    }

    #[test]
    fn returns_none_before_url_appears() {
        assert_eq!(parse_quick_url("INF Starting tunnel\nINF Connecting"), None);
        assert_eq!(parse_quick_url(""), None);
    }

    #[test]
    fn bare_apex_is_not_a_valid_tunnel_host() {
        assert_eq!(parse_quick_url("https://trycloudflare.com"), None);
    }

    #[test]
    fn named_ready_markers_match() {
        assert!(is_named_ready(
            "INF Registered tunnel connection connIndex=0 location=lhr"
        ));
        assert!(is_named_ready("Connection registered connIndex=1"));
        assert!(!is_named_ready("INF Starting tunnel"));
    }

    #[test]
    fn finds_login_auth_url() {
        let chunk = "Please open the following URL and log in:\n\
            https://dash.cloudflare.com/argotunnel?callback=https%3A%2F%2Flogin.example .";
        assert_eq!(
            find_auth_url(chunk).as_deref(),
            Some("https://dash.cloudflare.com/argotunnel?callback=https%3A%2F%2Flogin.example")
        );
    }

    #[test]
    fn login_url_absent_yields_none() {
        assert_eq!(find_auth_url("INF waiting for login"), None);
    }

    #[test]
    fn finds_tunnel_id_from_create_output() {
        let out = "Created tunnel mysite with id 6ff42ae2-765d-4adf-8112-31c55c1551ef\n";
        assert_eq!(
            find_tunnel_id(out).as_deref(),
            Some("6ff42ae2-765d-4adf-8112-31c55c1551ef")
        );
    }

    #[test]
    fn rejects_non_uuid() {
        assert_eq!(find_tunnel_id("no id here, just words and 12345"), None);
        assert!(!is_uuid("6ff42ae2-765d-4adf-8112-31c55c1551e")); // 11 in last group
        assert!(!is_uuid("zzzzzzzz-765d-4adf-8112-31c55c1551ef")); // non-hex
        assert!(is_uuid("6ff42ae2-765d-4adf-8112-31c55c1551ef"));
    }
}
