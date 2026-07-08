//! Pure helpers for "unbound" (resolver-off) localhost access.
//!
//! When the OS `.test` resolver isn't installed, sites are reached over
//! `http://localhost:8080`. Three mechanisms let a request select a site:
//!
//! 1. **`X-Yerd-Site: app.test`** header - for API clients: route this one
//!    request to that site, no cookie, no redirect.
//! 2. **`/~app.test/<path>`** switch URL - pins the origin to the site with a
//!    `yerd-site` cookie and redirects (303) to `<path>`.
//! 3. The **`yerd-site` cookie** - subsequent requests on the pinned origin.
//!
//! With none of those, a browser navigation gets a **picker page** listing the
//! sites; selecting one forwards the originally-requested path.
//!
//! A pinned browser can get back to the picker via the bare **`/~`** path,
//! which clears the pin cookie and redirects to `/` - an escape hatch, not a
//! fourth selection mechanism.
//!
//! Everything here is synchronous, I/O-free, and unit-tested. The orchestration
//! that needs the live [`yerd_core::SiteRouter`] and async drop ordering lives
//! in [`crate::server`].

use std::borrow::Cow;

/// Name of the cookie that pins the localhost origin to a site (its value is the
/// site's label, e.g. `app`).
pub const SITE_COOKIE: &str = "yerd-site";

/// Canonical per-request header that targets a site directly (value is a domain
/// like `app.test` or a bare site label). Intended for API clients that can't
/// follow the cookie/redirect dance. HTTP header lookup is case-insensitive.
pub const SITE_HEADER: &str = "x-yerd-site";

/// Dash-free alias for [`SITE_HEADER`] (`X-Yerdsite`), accepted as a convenience.
pub const SITE_HEADER_ALIAS: &str = "x-yerdsite";

/// The result of parsing a `/~<domain>[/rest]` switch path.
pub struct SwitchParse<'a> {
    /// The embedded domain, e.g. `app.test`.
    pub domain: &'a str,
    /// The remainder path, always beginning with `/` (e.g. `/foo` or `/`).
    pub remainder: &'a str,
}

/// One site as presented in the picker page.
pub struct PickerSite<'a> {
    /// Site label (no TLD), e.g. `app`. Used only as the switch-pin identity.
    pub name: &'a str,
    /// The site's primary domain FQDN, e.g. `app.test` or `corp.test`. The picker
    /// links to and displays this (the apex may not route once customised).
    pub primary: &'a str,
    /// Whether the site is configured for HTTPS (shown as a badge; it is still
    /// served over plain http in unbound mode).
    pub secure: bool,
    /// Human label for the site kind, e.g. `parked` / `linked`.
    pub kind: &'a str,
}

/// Returns `true` when `raw_host` (a `Host:` header value, optionally with a
/// port) is a loopback name we serve unbound traffic on: `localhost`,
/// `127.0.0.1`, or the IPv6 `::1`.
#[must_use]
pub fn is_loopback_host(raw_host: &str) -> bool {
    let bare = strip_host_port(raw_host);
    let bare = bare
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(bare);
    bare.eq_ignore_ascii_case("localhost") || bare == "127.0.0.1" || bare == "::1"
}

/// Strip a trailing `:port` from a host. Mirrors the proxy's existing
/// `redirect::strip_port` rules: bracketed IPv6 keeps its brackets; an
/// unbracketed value is only split when it has exactly one colon (so a bare
/// `::1` literal is left intact).
fn strip_host_port(host: &str) -> &str {
    if let Some(rest) = host.strip_prefix('[') {
        return match rest.find(']') {
            Some(end) => host.get(..end + 2).unwrap_or(host),
            None => host,
        };
    }
    let colons = host.bytes().filter(|&b| b == b':').count();
    if colons == 1 {
        host.split(':').next().unwrap_or(host)
    } else {
        host
    }
}

/// Parse a `/~<domain>[/rest]` switch path. Returns `None` when `path` does not
/// start with `/~` or the domain is empty (`/~`, `/~/x`).
#[must_use]
pub fn parse_switch(path: &str) -> Option<SwitchParse<'_>> {
    let after = path.strip_prefix("/~")?;
    let (domain, remainder) = match after.find('/') {
        Some(i) => {
            let (d, r) = after.split_at(i);
            (d, r)
        }
        None => (after, "/"),
    };
    if domain.is_empty() {
        return None;
    }
    Some(SwitchParse { domain, remainder })
}

/// Returns `true` for the bare "back to picker" path (`/~` or `/~/`), as
/// opposed to `/~<domain>` (see [`parse_switch`]). Checked ahead of the pin
/// cookie so it always wins and clears an existing pin.
#[must_use]
pub fn is_clear_switch(path: &str) -> bool {
    matches!(path, "/~" | "/~/")
}

/// Extract the `yerd-site` value from a `Cookie:` header, tolerating other
/// cookies. Returns `None` when absent or empty.
#[must_use]
pub fn parse_cookie_site(cookie_header: &str) -> Option<&str> {
    cookie_header.split(';').find_map(|pair| {
        let (k, v) = pair.trim().split_once('=')?;
        (k == SITE_COOKIE && !v.is_empty()).then_some(v)
    })
}

/// Normalise a request path into a guaranteed **same-origin absolute path**:
/// exactly one leading `/`, with any leading run of `/` or `\` collapsed to it.
///
/// This stops a protocol-relative (`//evil.com`) or backslash (`/\evil.com`,
/// which some browsers fold into `//`) destination from smuggling an off-origin
/// target into a redirect `Location` or a picker `href` - i.e. it closes the
/// open-redirect vector on the `/~switch` path.
#[must_use]
pub fn sanitize_dest(path: &str) -> Cow<'_, str> {
    let trimmed = path.trim_start_matches(['/', '\\']);
    let stripped = path.len() - trimmed.len();
    if stripped == 1 && path.starts_with('/') {
        return Cow::Borrowed(path);
    }
    if trimmed.is_empty() {
        return Cow::Borrowed("/");
    }
    Cow::Owned(format!("/{trimmed}"))
}

/// Build the `Set-Cookie` value that pins the origin to `site_name`.
///
/// No `Secure` attribute (the origin is plain http), session-scoped (no
/// `Max-Age`). `site_name` is a validated site label (`[a-z0-9-]`, dot-free) so
/// it needs no escaping.
#[must_use]
pub fn build_set_cookie(site_name: &str) -> String {
    format!("{SITE_COOKIE}={site_name}; Path=/; HttpOnly; SameSite=Lax")
}

/// Build the `Set-Cookie` value that clears a stale pin (`Max-Age=0`).
#[must_use]
pub fn build_clear_cookie() -> String {
    format!("{SITE_COOKIE}=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax")
}

/// HTML-escape `&`, `<`, `>`, `"`, `'` for safe interpolation into element text
/// and double-quoted attributes. Borrows the input when nothing needs escaping.
#[must_use]
pub fn html_escape(s: &str) -> Cow<'_, str> {
    if s.bytes()
        .any(|b| matches!(b, b'&' | b'<' | b'>' | b'"' | b'\''))
    {
        let mut out = String::with_capacity(s.len() + 16);
        for c in s.chars() {
            match c {
                '&' => out.push_str("&amp;"),
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                '"' => out.push_str("&quot;"),
                '\'' => out.push_str("&#39;"),
                other => out.push(other),
            }
        }
        Cow::Owned(out)
    } else {
        Cow::Borrowed(s)
    }
}

/// Render the branded site-picker page.
///
/// `dest` is the originally-requested path+query (escaped before use); each site
/// links to `/~{primary}{dest}` (its primary domain FQDN) so the path is
/// forwarded on select. The page is fully self-contained (inline CSS + SVG, no
/// JS, no external assets) and themes itself via `prefers-color-scheme`.
#[must_use]
pub fn render_picker(sites: &[PickerSite<'_>], dest: &str) -> String {
    let dest_e = html_escape(dest);
    let dest_e = dest_e.as_ref();

    let body = if sites.is_empty() {
        "<p class=\"empty\">No sites yet — park or link a directory first.</p>".to_owned()
    } else {
        use std::fmt::Write as _;
        let mut list = String::from("<div class=\"list\">");
        for s in sites {
            let primary = html_escape(s.primary);
            let primary = primary.as_ref();
            let kind = html_escape(s.kind);
            let kind = kind.as_ref();
            let badge = if s.secure {
                "<span class=\"badge\" title=\"configured for HTTPS — served over plain http here\">secure</span>"
            } else {
                ""
            };
            let _ = write!(
                list,
                "<a class=\"site\" href=\"/~{primary}{dest_e}\">\
                   <span class=\"site-name\">{primary}</span>\
                   <span class=\"site-meta\">{kind}{badge}</span>\
                 </a>",
            );
        }
        list.push_str("</div>");
        list
    };

    format!(
        "<!doctype html>\
<html lang=\"en\">\
<head>\
<meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<meta name=\"color-scheme\" content=\"light dark\">\
<title>Yerd — choose a site</title>\
<style>{PICKER_CSS}</style>\
</head>\
<body>\
<main class=\"wrap\">\
<div class=\"brand\">{PICKER_LOGO}<h1>Yerd</h1></div>\
<p class=\"sub\">Choose a site to open.</p>\
{body}\
<p class=\"opening\">Opening <code>{dest_e}</code></p>\
<p class=\"note\">Served over plain <code>http://localhost</code>. Sites that \
force HTTPS may not load.</p>\
</main>\
</body>\
</html>"
    )
}

/// Inline Yerd logomark (the brand "Y" on the indigo gradient squircle). The
/// gradient `id` is namespaced to avoid collisions if the page is ever embedded.
const PICKER_LOGO: &str = "<svg viewBox=\"0 0 256 256\" xmlns=\"http://www.w3.org/2000/svg\" role=\"img\" aria-label=\"Yerd\">\
<defs><linearGradient id=\"yerd-pick-bg\" x1=\"0.125\" y1=\"0.0625\" x2=\"0.875\" y2=\"0.9375\">\
<stop stop-color=\"#6366F1\"/><stop offset=\"1\" stop-color=\"#4338CA\"/>\
</linearGradient></defs>\
<rect width=\"256\" height=\"256\" rx=\"60\" fill=\"url(#yerd-pick-bg)\"/>\
<path d=\"M82 76 L128 140 L174 76 M128 140 L128 188\" stroke=\"#fff\" stroke-width=\"30\" stroke-linecap=\"round\" stroke-linejoin=\"round\"/>\
</svg>";

/// Picker stylesheet. Tokens mirror the app's `style.css` (HSL) with a
/// `prefers-color-scheme: dark` override.
const PICKER_CSS: &str = ":root{--bg:hsl(0 0% 100%);--fg:hsl(0 0% 9%);--card:hsl(0 0% 100%);--border:hsl(0 0% 90%);--muted:hsl(0 0% 40%);--brand:hsl(239 84% 67%)}\
@media (prefers-color-scheme:dark){:root{--bg:hsl(0 0% 11%);--fg:hsl(0 0% 98%);--card:hsl(0 0% 14.5%);--border:hsl(0 0% 20%);--muted:hsl(0 0% 64%);--brand:hsl(239 84% 72%)}}\
*{box-sizing:border-box}\
body{margin:0;background:var(--bg);color:var(--fg);font:15px/1.5 -apple-system,BlinkMacSystemFont,\"Segoe UI\",Roboto,Helvetica,Arial,sans-serif}\
.wrap{max-width:560px;margin:0 auto;padding:48px 20px}\
.brand{display:flex;align-items:center;gap:12px}\
.brand svg{width:40px;height:40px;border-radius:10px;display:block}\
.brand h1{font-size:20px;margin:0;font-weight:650;letter-spacing:-.01em}\
.sub{color:var(--muted);margin:4px 0 24px}\
.list{display:flex;flex-direction:column;gap:8px}\
.site{display:flex;align-items:center;justify-content:space-between;gap:12px;padding:14px 16px;border:1px solid var(--border);border-radius:12px;background:var(--card);color:inherit;text-decoration:none;transition:border-color .12s,transform .12s}\
.site:hover,.site:focus-visible{border-color:var(--brand);transform:translateY(-1px);outline:none}\
.site-name{font-weight:550}\
.site-meta{display:flex;align-items:center;gap:8px;color:var(--muted);font-size:13px}\
.badge{border:1px solid var(--border);border-radius:999px;padding:1px 8px;font-size:11px;color:var(--brand)}\
.empty{color:var(--muted);border:1px dashed var(--border);border-radius:12px;padding:24px;text-align:center}\
.opening{color:var(--muted);font-size:13px;margin:20px 0 0}\
.opening code,.note code{border:1px solid var(--border);padding:1px 6px;border-radius:6px}\
.note{color:var(--muted);font-size:12px;margin:8px 0 0;line-height:1.4}";

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
    fn loopback_host_table() {
        for ok in [
            "localhost",
            "localhost:8080",
            "LOCALHOST",
            "127.0.0.1",
            "127.0.0.1:8080",
            "[::1]",
            "[::1]:8080",
            "::1",
        ] {
            assert!(is_loopback_host(ok), "{ok:?} should be loopback");
        }
        for no in ["app.test", "app.test:8080", "1.2.3.4", "127.0.0.2", ""] {
            assert!(!is_loopback_host(no), "{no:?} should not be loopback");
        }
    }

    #[test]
    fn parse_switch_table() {
        let cases: &[(&str, Option<(&str, &str)>)] = &[
            ("/~app.test", Some(("app.test", "/"))),
            ("/~app.test/", Some(("app.test", "/"))),
            ("/~app.test/foo", Some(("app.test", "/foo"))),
            ("/~app.test/foo/bar", Some(("app.test", "/foo/bar"))),
            ("/~app", Some(("app", "/"))),
            ("/~", None),
            ("/~/x", None),
            ("/foo", None),
            ("/", None),
        ];
        for (path, want) in cases {
            let got = parse_switch(path).map(|s| (s.domain, s.remainder));
            assert_eq!(got, *want, "path {path:?}");
        }
    }

    #[test]
    fn is_clear_switch_table() {
        let cases: &[(&str, bool)] = &[
            ("/~", true),
            ("/~/", true),
            ("/~app.test", false),
            ("/~app", false),
            ("/~/x", false),
            ("/", false),
            ("/foo", false),
        ];
        for (path, want) in cases {
            assert_eq!(is_clear_switch(path), *want, "path {path:?}");
        }
    }

    #[test]
    fn sanitize_dest_collapses_off_origin_prefixes() {
        let cases: &[(&str, &str)] = &[
            ("/", "/"),
            ("/foo", "/foo"),
            ("/foo/bar?x=1", "/foo/bar?x=1"),
            ("/foo//bar", "/foo//bar"),
            ("//evil.com", "/evil.com"),
            ("///evil.com", "/evil.com"),
            ("/\\evil.com", "/evil.com"),
            ("/\\/evil.com", "/evil.com"),
            ("\\evil.com", "/evil.com"),
            ("//", "/"),
            ("", "/"),
        ];
        for (input, want) in cases {
            assert_eq!(sanitize_dest(input), *want, "input {input:?}");
        }
    }

    #[test]
    fn cookie_parse() {
        assert_eq!(parse_cookie_site("yerd-site=app"), Some("app"));
        assert_eq!(parse_cookie_site("a=1; yerd-site=app; b=2"), Some("app"));
        assert_eq!(parse_cookie_site("  yerd-site=app  "), Some("app"));
        assert_eq!(parse_cookie_site("other=1"), None);
        assert_eq!(parse_cookie_site("yerd-site="), None);
        assert_eq!(parse_cookie_site(""), None);
    }

    #[test]
    fn cookie_build_shapes() {
        let set = build_set_cookie("app");
        assert!(set.contains("yerd-site=app"));
        assert!(set.contains("Path=/"));
        assert!(set.contains("HttpOnly"));
        assert!(set.contains("SameSite=Lax"));
        assert!(!set.contains("Secure"));
        let clear = build_clear_cookie();
        assert!(clear.contains("Max-Age=0"));
        assert!(clear.contains("yerd-site=;"));
    }

    #[test]
    fn html_escape_cases() {
        assert_eq!(html_escape("plain"), Cow::Borrowed("plain"));
        assert_eq!(
            html_escape("a<b>&\"'"),
            "a&lt;b&gt;&amp;&quot;&#39;".to_owned()
        );
    }

    #[test]
    fn picker_lists_sites_by_primary_and_forwards_dest() {
        let sites = [
            PickerSite {
                name: "app",
                primary: "app.test",
                secure: false,
                kind: "linked",
            },
            PickerSite {
                name: "blog",
                primary: "corp.test",
                secure: true,
                kind: "parked",
            },
        ];
        let html = render_picker(&sites, "/example?x=1");
        assert!(html.contains("href=\"/~app.test/example?x=1\""));
        // The picker links to the primary domain, not the bare name apex.
        assert!(html.contains("href=\"/~corp.test/example?x=1\""));
        assert!(html.contains(">corp.test<"));
        assert_eq!(html.matches("class=\"badge\"").count(), 1);
        assert!(html.contains("prefers-color-scheme:dark"));
    }

    #[test]
    fn picker_escapes_malicious_dest() {
        let sites = [PickerSite {
            name: "app",
            primary: "app.test",
            secure: false,
            kind: "linked",
        }];
        let html = render_picker(&sites, "/x\"><script>alert(1)</script>");
        assert!(!html.contains("\"><script>"));
        assert!(html.contains("&quot;&gt;&lt;script&gt;"));
    }

    #[test]
    fn picker_empty_state() {
        let html = render_picker(&[], "/");
        assert!(html.contains("No sites yet"));
        assert!(!html.contains("class=\"site\""));
    }
}
