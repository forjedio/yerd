//! Pure helpers for the tunnel origin listener: public `Host` normalization and
//! the HTTP basic-auth share-password check.

/// Normalize a `Host` header value into a tunnel route key: strip a `:port`
/// suffix and a trailing dot, and lowercase. Returns `None` for an empty value
/// or one containing whitespace.
#[must_use]
pub fn normalize_host(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_whitespace) {
        return None;
    }
    let without_port = match trimmed.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => host,
        Some(_) => return None,
        None => trimmed,
    };
    let host = without_port.strip_suffix('.').unwrap_or(without_port);
    if host.is_empty() {
        return None;
    }
    Some(host.to_ascii_lowercase())
}

/// Extract the password from an `Authorization: Basic <base64(user:pass)>`
/// header. The scheme is matched case-insensitively and the credentials are
/// split at the first `:`. **The username is ignored**: a share password works
/// with any username. Returns `None` for any other scheme, undecodable base64,
/// non-UTF-8 credentials, a missing `:`, or an empty password.
#[must_use]
pub fn basic_auth_password(header: &str) -> Option<String> {
    let (scheme, token) = header.trim().split_once(char::is_whitespace)?;
    if !scheme.eq_ignore_ascii_case("basic") {
        return None;
    }
    let decoded = crate::pure::base64::decode(token.trim())?;
    let credentials = String::from_utf8(decoded).ok()?;
    let (_user, password) = credentials.split_once(':')?;
    if password.is_empty() {
        return None;
    }
    Some(password.to_owned())
}

/// Constant-time comparison of a supplied share password against the expected
/// one, so the check does not leak how many leading bytes matched.
#[must_use]
pub fn password_matches(given: &str, expected: &str) -> bool {
    use subtle::ConstantTimeEq as _;
    given.as_bytes().ct_eq(expected.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_host_table() {
        for (raw, expected) in [
            ("app.example.com", Some("app.example.com")),
            ("A.Example.com.", Some("a.example.com")),
            ("app.example.com:443", Some("app.example.com")),
            ("App.Example.Com.:8080", Some("app.example.com")),
            (" app.example.com ", Some("app.example.com")),
            ("", None),
            ("   ", None),
            (":443", None),
            (".", None),
            ("app example.com", None),
            ("app.example.com:", None),
            ("app.example.com:abc", None),
        ] {
            assert_eq!(normalize_host(raw).as_deref(), expected, "input {raw:?}");
        }
    }

    #[test]
    fn basic_auth_password_table() {
        for (header, expected) in [
            ("Basic dXNlcjpzZWNyZXQ=", Some("secret")),
            ("basic dXNlcjpzZWNyZXQ=", Some("secret")),
            ("BASIC   dXNlcjpzZWNyZXQ=", Some("secret")),
            ("Basic OnNlY3JldA==", Some("secret")),
            ("Basic dTpwOmFzcw==", Some("p:ass")),
            ("basic xyz", None),
            ("Basic dXNlcnNlY3JldA==", None),
            ("Basic dXNlcjo=", None),
            ("Basic not*base64", None),
            ("Bearer dXNlcjpzZWNyZXQ=", None),
            ("Basic", None),
            ("", None),
        ] {
            assert_eq!(
                basic_auth_password(header).as_deref(),
                expected,
                "header {header:?}"
            );
        }
    }

    #[test]
    fn password_matches_table() {
        for (given, expected, ok) in [
            ("secret", "secret", true),
            ("secreT", "secret", false),
            ("secre", "secret", false),
            ("secrets", "secret", false),
            ("", "secret", false),
            ("p\u{e9}ss", "p\u{e9}ss", true),
        ] {
            assert_eq!(
                password_matches(given, expected),
                ok,
                "{given:?} vs {expected:?}"
            );
        }
    }
}
