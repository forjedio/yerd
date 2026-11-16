//! The origin every tunnel forwards to: Yerd's loopback-only tunnel listener.
//!
//! The daemon serves a dedicated plain-HTTP listener on `127.0.0.1` that routes
//! by the **public** `Host` header through its own `public host -> site` map and
//! presents each request to PHP as HTTPS. So `cloudflared` needs no `Host`
//! rewrite, no SNI, and no TLS options: every tunnel kind, and every site on the
//! named tunnel, points at the same `http://127.0.0.1:<port>`.

/// The `service` URL `cloudflared` connects to for the tunnel listener bound on
/// loopback `port`, e.g. `http://127.0.0.1:49152`.
#[must_use]
pub fn origin_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_url_table() {
        for (port, expected) in [
            (1u16, "http://127.0.0.1:1"),
            (49152, "http://127.0.0.1:49152"),
            (u16::MAX, "http://127.0.0.1:65535"),
        ] {
            assert_eq!(origin_url(port), expected);
        }
    }
}
