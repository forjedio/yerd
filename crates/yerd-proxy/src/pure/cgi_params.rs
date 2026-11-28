//! Build the CGI parameter list for one FastCGI request.
//!
//! Policy: a `try_files`-style front controller. The caller (`forward::
//! script_file::resolve_script`) resolves the request path against the real
//! filesystem first - an exact `.php` match (`/wp-login.php`), or a
//! directory's own `index.php` (`/wp-admin/` -> `wp-admin/index.php`) - and
//! passes the result in as `script_rel`. When it finds a real script:
//!
//! - `SCRIPT_FILENAME = document_root / <script_rel>`
//! - `SCRIPT_NAME     = "/" + <script_rel>`
//!
//! Otherwise (Caddy-style "everything to index.php", the original MVP
//! policy and still correct for single-front-controller frameworks like
//! Laravel):
//!
//! - `SCRIPT_FILENAME = document_root / "index.php"`
//! - `SCRIPT_NAME     = "/index.php"`
//!
//! `PATH_INFO` is `<original path>` in both cases - WordPress and
//! Laravel both route on `REQUEST_URI`, not `PATH_INFO`, so leaving it as the
//! full original path (rather than splitting "extra path after the script",
//! full CGI/1.1 `PATH_INFO` semantics) keeps this a minimal, low-risk change
//! on top of already-pinned behavior. The exception is a `PATH_INFO`-style
//! request the resolver *split* (`/styles.php/extra` - see
//! `forward::script_file::ScriptResolution::ScriptWithPathInfo`): there the
//! caller passes the split remainder as `path_info` and `PATH_INFO` carries
//! exactly that, which is what nginx's `fastcgi_split_path_info` produces and
//! what Moodle-style slash-argument routing reads.
//!
//! Plus the standard CGI/1.1 vars and `HTTP_*`-translated headers.
//!
//! `SERVER_SOFTWARE` is deliberately `"yerd (nginx-compatible)"`, not just
//! `"yerd"`: frameworks (WordPress in particular - see `got_url_rewrite()` /
//! `$is_nginx` in wp-admin/includes/misc.php and wp-includes/vars.php) parse
//! this CGI var for known-good server names to decide whether extension-less
//! "pretty" URLs are safe to offer, since a plain front-controller fallback
//! isn't universal. yerd's front-controller resolution
//! (`forward::script_file::resolve_script`) is exactly nginx's classic
//! `try_files $uri $uri/ /index.php` policy, so this is an accurate capability
//! signal, not a spoof - and it's this CGI var PHP sees, not the client-facing
//! `Server:` HTTP header ([`yerd_core::PROXY_SERVER_ID`]), which still
//! identifies yerd honestly to browsers and tools.

use std::net::SocketAddr;
use std::path::Path;

/// A per-request `auto_prepend_file` override plus the custom param its
/// script gates on. One enum (rather than parallel optional parameters) so a
/// request can never carry two prepends, and "a gate param with no prepend
/// script" is unrepresentable.
#[derive(Debug, Clone, Copy)]
pub enum PrependParams<'a> {
    /// The one-click `WordPress` login flow - present only for the one request
    /// that already proved it holds a valid, now-consumed login token (see
    /// `dispatch` in `server.rs`). Emits `YERD_LOGIN_USER`.
    AutoLogin {
        /// Path to the `auto_prepend_file` bootstrap script.
        prepend_script: &'a Path,
        /// The `WordPress` login/username to sign in as, or `""` for no
        /// preference (the prepend script falls back to the earliest-created
        /// administrator).
        target_user: &'a str,
    },
    /// The tunnel's `WordPress` public-URL override - present on tunnelled
    /// requests to a `WordPress` site. Emits `YERD_PUBLIC_URL`.
    PublicUrl {
        /// Path to the `auto_prepend_file` script.
        prepend_script: &'a Path,
        /// The public origin (`https://<public host>`) to present as `home`
        /// and `siteurl`.
        url: &'a str,
    },
}

/// FastCGI param carrying the Insights request id (contract C2).
pub const TRACE_ID_PARAM: &str = "YERD_TRACE_ID";
/// FastCGI param carrying the dump-server loopback port (contract C2).
pub const TRACE_PORT_PARAM: &str = "YERD_TRACE_PORT";
/// FastCGI param carrying the Yerd site name (contract C2).
pub const TRACE_SITE_PARAM: &str = "YERD_TRACE_SITE";
/// FastCGI param carrying the comma-separated capture features (contract C2).
pub const TRACE_FEATURES_PARAM: &str = "YERD_TRACE_FEATURES";

/// The per-request Insights trace params handed to the `yerd-php-ext`
/// extension. None of the names start with `HTTP_`, so a client cannot
/// inject them through request headers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TraceParams<'a> {
    /// The request id: 32 lowercase hex chars, a 16-hex boot nonce followed
    /// by a 16-hex counter.
    pub id: &'a str,
    /// The dump server's loopback port.
    pub port: u16,
    /// The Yerd site name.
    pub site: &'a str,
    /// Comma-separated categories to emit, possibly empty.
    pub features: &'a str,
}

/// Build the CGI parameter pairs. `script_rel`, if given, is a real,
/// on-disk `.php` file's path relative to `document_root` (see the module
/// doc) - `None` falls back to the root `index.php` policy. `path_info`, if
/// given, is the decoded remainder of a `PATH_INFO`-style split resolved by
/// `forward::script_file` and overrides the default `PATH_INFO` value (the
/// full request path). `prepend`, if given, adds a
/// `PHP_VALUE: auto_prepend_file=<path>` param plus the custom param its
/// script gates on (`YERD_LOGIN_USER` or `YERD_PUBLIC_URL`) - see
/// [`PrependParams`]. `trace`, if given, appends the four `YERD_TRACE_*`
/// params after everything else (see [`TraceParams`]).
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn build_params(
    method: &str,
    path_and_query: &str,
    headers: &http::HeaderMap,
    document_root: &Path,
    script_rel: Option<&Path>,
    path_info: Option<&str>,
    https: bool,
    remote_addr: SocketAddr,
    server_addr: SocketAddr,
    prepend: Option<PrependParams<'_>>,
    trace: Option<TraceParams<'_>>,
) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut out: Vec<(Vec<u8>, Vec<u8>)> = Vec::with_capacity(16 + headers.len());

    let (path, query) = split_path_query(path_and_query);
    let (script_filename, script_name) = match script_rel {
        Some(rel) => (
            document_root.join(rel),
            format!("/{}", rel.to_string_lossy().replace('\\', "/")),
        ),
        None => (document_root.join("index.php"), "/index.php".to_owned()),
    };

    push(&mut out, b"GATEWAY_INTERFACE", b"CGI/1.1");
    push(&mut out, b"SERVER_PROTOCOL", b"HTTP/1.1");
    push(&mut out, b"REQUEST_METHOD", method.as_bytes());
    push(&mut out, b"REQUEST_URI", path_and_query.as_bytes());
    push(&mut out, b"QUERY_STRING", query.as_bytes());
    push(&mut out, b"SCRIPT_NAME", script_name.as_bytes());
    push(
        &mut out,
        b"SCRIPT_FILENAME",
        script_filename.to_string_lossy().as_bytes(),
    );
    push(
        &mut out,
        b"DOCUMENT_ROOT",
        document_root.to_string_lossy().as_bytes(),
    );
    push(&mut out, b"PATH_INFO", path_info.unwrap_or(path).as_bytes());
    push(
        &mut out,
        b"REMOTE_ADDR",
        remote_addr.ip().to_string().as_bytes(),
    );
    push(
        &mut out,
        b"REMOTE_PORT",
        remote_addr.port().to_string().as_bytes(),
    );
    push(
        &mut out,
        b"SERVER_ADDR",
        server_addr.ip().to_string().as_bytes(),
    );
    push(
        &mut out,
        b"SERVER_PORT",
        server_addr.port().to_string().as_bytes(),
    );
    push(&mut out, b"SERVER_SOFTWARE", b"yerd (nginx-compatible)");
    if https {
        push(&mut out, b"HTTPS", b"on");
    }
    if let Some(prepend) = prepend {
        push_prepend(&mut out, prepend);
    }

    if let Some(host) = headers
        .get(http::header::HOST)
        .and_then(|v| v.to_str().ok())
    {
        push(&mut out, b"SERVER_NAME", host.as_bytes());
        push(&mut out, b"HTTP_HOST", host.as_bytes());
    }
    if let Some(ct) = headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
    {
        push(&mut out, b"CONTENT_TYPE", ct.as_bytes());
    }
    if let Some(cl) = headers
        .get(http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
    {
        push(&mut out, b"CONTENT_LENGTH", cl.as_bytes());
    }

    for (name, value) in headers {
        if matches!(
            name,
            &http::header::HOST | &http::header::CONTENT_TYPE | &http::header::CONTENT_LENGTH
        ) {
            continue;
        }
        let mut key = b"HTTP_".to_vec();
        for byte in name.as_str().as_bytes() {
            key.push(if *byte == b'-' {
                b'_'
            } else {
                byte.to_ascii_uppercase()
            });
        }
        push(&mut out, &key, value.as_bytes());
    }

    if let Some(trace) = trace {
        push_trace(&mut out, trace);
    }

    out
}

fn push(out: &mut Vec<(Vec<u8>, Vec<u8>)>, name: &[u8], value: &[u8]) {
    out.push((name.to_vec(), value.to_vec()));
}

fn push_prepend(out: &mut Vec<(Vec<u8>, Vec<u8>)>, prepend: PrependParams<'_>) {
    let (prepend_script, gate_name, gate_value): (&Path, &[u8], &str) = match prepend {
        PrependParams::AutoLogin {
            prepend_script,
            target_user,
        } => (prepend_script, b"YERD_LOGIN_USER", target_user),
        PrependParams::PublicUrl {
            prepend_script,
            url,
        } => (prepend_script, b"YERD_PUBLIC_URL", url),
    };
    push(
        out,
        b"PHP_VALUE",
        format!("auto_prepend_file={}", prepend_script.display()).as_bytes(),
    );
    push(out, gate_name, gate_value.as_bytes());
}

fn push_trace(out: &mut Vec<(Vec<u8>, Vec<u8>)>, trace: TraceParams<'_>) {
    push(out, TRACE_ID_PARAM.as_bytes(), trace.id.as_bytes());
    push(
        out,
        TRACE_PORT_PARAM.as_bytes(),
        trace.port.to_string().as_bytes(),
    );
    push(out, TRACE_SITE_PARAM.as_bytes(), trace.site.as_bytes());
    push(
        out,
        TRACE_FEATURES_PARAM.as_bytes(),
        trace.features.as_bytes(),
    );
}

fn split_path_query(path_and_query: &str) -> (&str, &str) {
    match path_and_query.split_once('?') {
        Some((p, q)) => (p, q),
        None => (path_and_query, ""),
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
    use http::HeaderMap;
    use std::path::PathBuf;

    fn lookup<'a>(pairs: &'a [(Vec<u8>, Vec<u8>)], key: &[u8]) -> Option<&'a [u8]> {
        pairs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_slice())
    }

    fn make_headers(host: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(http::header::HOST, host.parse().unwrap());
        h
    }

    #[test]
    fn caddy_style_everything_to_index_php() {
        let root = PathBuf::from("/srv/www/app");
        let pairs = build_params(
            "GET",
            "/foo/bar?a=1&b=2",
            &make_headers("app.test"),
            &root,
            None,
            None,
            false,
            "127.0.0.1:54321".parse().unwrap(),
            "127.0.0.1:80".parse().unwrap(),
            None,
            None,
        );
        assert_eq!(
            lookup(&pairs, b"SCRIPT_NAME"),
            Some(b"/index.php".as_slice())
        );
        assert_eq!(
            lookup(&pairs, b"SCRIPT_FILENAME"),
            Some("/srv/www/app/index.php".as_bytes())
        );
        assert_eq!(lookup(&pairs, b"PATH_INFO"), Some(b"/foo/bar".as_slice()));
        assert_eq!(
            lookup(&pairs, b"REQUEST_URI"),
            Some(b"/foo/bar?a=1&b=2".as_slice())
        );
        assert_eq!(lookup(&pairs, b"QUERY_STRING"), Some(b"a=1&b=2".as_slice()));
        assert_eq!(lookup(&pairs, b"REQUEST_METHOD"), Some(b"GET".as_slice()));
        assert_eq!(lookup(&pairs, b"SERVER_NAME"), Some(b"app.test".as_slice()));
        assert_eq!(lookup(&pairs, b"HTTP_HOST"), Some(b"app.test".as_slice()));
        assert_eq!(
            lookup(&pairs, b"DOCUMENT_ROOT"),
            Some(b"/srv/www/app".as_slice())
        );
        assert!(lookup(&pairs, b"HTTPS").is_none());
    }

    #[test]
    fn server_software_contains_nginx_for_framework_rewrite_detection() {
        // WordPress (and other frameworks) gate "pretty"/extension-less
        // permalink options on this CGI var containing a known-good server
        // name - see the module doc for the full explanation.
        let pairs = build_params(
            "GET",
            "/",
            &make_headers("app.test"),
            Path::new("/srv"),
            None,
            None,
            false,
            "127.0.0.1:1".parse().unwrap(),
            "127.0.0.1:80".parse().unwrap(),
            None,
            None,
        );
        let software = String::from_utf8_lossy(lookup(&pairs, b"SERVER_SOFTWARE").unwrap());
        assert!(software.contains("nginx"), "got {software:?}");
    }

    #[test]
    fn web_root_subdir_drives_script_filename_and_document_root() {
        let mut site =
            yerd_core::Site::linked("app", "/srv/www/app", yerd_core::PhpVersion::new(8, 3))
                .unwrap();
        site.set_web_subpath("public");
        let served = site.served_root();
        let pairs = build_params(
            "GET",
            "/login",
            &make_headers("app.test"),
            &served,
            None,
            None,
            false,
            "127.0.0.1:1".parse().unwrap(),
            "127.0.0.1:80".parse().unwrap(),
            None,
            None,
        );
        assert_eq!(
            lookup(&pairs, b"DOCUMENT_ROOT"),
            Some("/srv/www/app/public".as_bytes())
        );
        assert_eq!(
            lookup(&pairs, b"SCRIPT_FILENAME"),
            Some("/srv/www/app/public/index.php".as_bytes())
        );
    }

    #[test]
    fn https_param_is_on_when_secure() {
        let pairs = build_params(
            "POST",
            "/",
            &make_headers("app.test"),
            Path::new("/srv/www/app"),
            None,
            None,
            true,
            "1.2.3.4:1000".parse().unwrap(),
            "127.0.0.1:443".parse().unwrap(),
            None,
            None,
        );
        assert_eq!(lookup(&pairs, b"HTTPS"), Some(b"on".as_slice()));
    }

    #[test]
    fn http_headers_translated_to_http_underscore() {
        let mut headers = make_headers("app.test");
        headers.insert("X-Custom", "yes".parse().unwrap());
        headers.insert(http::header::ACCEPT, "text/html".parse().unwrap());
        let pairs = build_params(
            "GET",
            "/",
            &headers,
            Path::new("/srv"),
            None,
            None,
            false,
            "127.0.0.1:1".parse().unwrap(),
            "127.0.0.1:80".parse().unwrap(),
            None,
            None,
        );
        assert_eq!(lookup(&pairs, b"HTTP_X_CUSTOM"), Some(b"yes".as_slice()));
        assert_eq!(
            lookup(&pairs, b"HTTP_ACCEPT"),
            Some(b"text/html".as_slice())
        );
    }

    #[test]
    fn content_type_and_length_pulled_out_of_http_prefix() {
        let mut headers = make_headers("app.test");
        headers.insert(
            http::header::CONTENT_TYPE,
            "application/json".parse().unwrap(),
        );
        headers.insert(http::header::CONTENT_LENGTH, "42".parse().unwrap());
        let pairs = build_params(
            "POST",
            "/",
            &headers,
            Path::new("/srv"),
            None,
            None,
            false,
            "127.0.0.1:1".parse().unwrap(),
            "127.0.0.1:80".parse().unwrap(),
            None,
            None,
        );
        assert_eq!(
            lookup(&pairs, b"CONTENT_TYPE"),
            Some(b"application/json".as_slice())
        );
        assert_eq!(lookup(&pairs, b"CONTENT_LENGTH"), Some(b"42".as_slice()));
        assert!(lookup(&pairs, b"HTTP_CONTENT_TYPE").is_none());
        assert!(lookup(&pairs, b"HTTP_CONTENT_LENGTH").is_none());
    }

    #[test]
    fn no_query_string_yields_empty_query() {
        let pairs = build_params(
            "GET",
            "/just/path",
            &make_headers("a.test"),
            Path::new("/srv"),
            None,
            None,
            false,
            "127.0.0.1:1".parse().unwrap(),
            "127.0.0.1:80".parse().unwrap(),
            None,
            None,
        );
        assert_eq!(lookup(&pairs, b"PATH_INFO"), Some(b"/just/path".as_slice()));
        assert_eq!(lookup(&pairs, b"QUERY_STRING"), Some(b"".as_slice()));
    }

    #[test]
    fn resolved_script_drives_script_name_and_filename() {
        let pairs = build_params(
            "GET",
            "/wp-admin/?page=1",
            &make_headers("blog.test"),
            Path::new("/srv/www/blog"),
            Some(Path::new("wp-admin/index.php")),
            None,
            false,
            "127.0.0.1:1".parse().unwrap(),
            "127.0.0.1:80".parse().unwrap(),
            None,
            None,
        );
        assert_eq!(
            lookup(&pairs, b"SCRIPT_NAME"),
            Some(b"/wp-admin/index.php".as_slice())
        );
        assert_eq!(
            lookup(&pairs, b"SCRIPT_FILENAME"),
            Some("/srv/www/blog/wp-admin/index.php".as_bytes())
        );
        // PATH_INFO stays the full original path either way - WordPress and
        // Laravel both route on REQUEST_URI, not PATH_INFO (see module doc).
        assert_eq!(lookup(&pairs, b"PATH_INFO"), Some(b"/wp-admin/".as_slice()));
    }

    #[test]
    fn auto_login_adds_prepend_and_target_user_params() {
        let pairs = build_params(
            "GET",
            "/wp-admin/",
            &make_headers("blog.test"),
            Path::new("/srv/www/blog"),
            None,
            None,
            false,
            "127.0.0.1:1".parse().unwrap(),
            "127.0.0.1:80".parse().unwrap(),
            Some(PrependParams::AutoLogin {
                prepend_script: Path::new("/data/wordpress-autologin-prepend.php"),
                target_user: "admin",
            }),
            None,
        );
        assert_eq!(
            lookup(&pairs, b"PHP_VALUE"),
            Some(b"auto_prepend_file=/data/wordpress-autologin-prepend.php".as_slice())
        );
        assert_eq!(
            lookup(&pairs, b"YERD_LOGIN_USER"),
            Some(b"admin".as_slice())
        );
    }

    #[test]
    fn auto_login_with_no_preference_sends_empty_target_user() {
        let pairs = build_params(
            "GET",
            "/wp-admin/",
            &make_headers("blog.test"),
            Path::new("/srv/www/blog"),
            None,
            None,
            false,
            "127.0.0.1:1".parse().unwrap(),
            "127.0.0.1:80".parse().unwrap(),
            Some(PrependParams::AutoLogin {
                prepend_script: Path::new("/data/wordpress-autologin-prepend.php"),
                target_user: "",
            }),
            None,
        );
        assert_eq!(lookup(&pairs, b"YERD_LOGIN_USER"), Some(b"".as_slice()));
    }

    #[test]
    fn no_auto_login_omits_prepend_and_target_user_params() {
        let pairs = build_params(
            "GET",
            "/",
            &make_headers("app.test"),
            Path::new("/srv/www/app"),
            None,
            None,
            false,
            "127.0.0.1:1".parse().unwrap(),
            "127.0.0.1:80".parse().unwrap(),
            None,
            None,
        );
        assert!(lookup(&pairs, b"PHP_VALUE").is_none());
        assert!(lookup(&pairs, b"YERD_LOGIN_USER").is_none());
        assert!(lookup(&pairs, b"YERD_PUBLIC_URL").is_none());
    }

    #[test]
    fn public_url_adds_prepend_and_public_url_params_only() {
        let pairs = build_params(
            "GET",
            "/",
            &make_headers("app.example.com"),
            Path::new("/srv/www/blog"),
            None,
            None,
            true,
            "127.0.0.1:1".parse().unwrap(),
            "127.0.0.1:443".parse().unwrap(),
            Some(PrependParams::PublicUrl {
                prepend_script: Path::new("/data/wordpress-public-url-prepend.php"),
                url: "https://app.example.com",
            }),
            None,
        );
        assert_eq!(
            lookup(&pairs, b"PHP_VALUE"),
            Some(b"auto_prepend_file=/data/wordpress-public-url-prepend.php".as_slice())
        );
        assert_eq!(
            lookup(&pairs, b"YERD_PUBLIC_URL"),
            Some(b"https://app.example.com".as_slice())
        );
        assert!(lookup(&pairs, b"YERD_LOGIN_USER").is_none());
    }

    #[test]
    fn resolved_exact_script_match_drives_script_name_and_filename() {
        let pairs = build_params(
            "POST",
            "/wp-login.php",
            &make_headers("blog.test"),
            Path::new("/srv/www/blog"),
            Some(Path::new("wp-login.php")),
            None,
            false,
            "127.0.0.1:1".parse().unwrap(),
            "127.0.0.1:80".parse().unwrap(),
            None,
            None,
        );
        assert_eq!(
            lookup(&pairs, b"SCRIPT_NAME"),
            Some(b"/wp-login.php".as_slice())
        );
        assert_eq!(
            lookup(&pairs, b"SCRIPT_FILENAME"),
            Some("/srv/www/blog/wp-login.php".as_bytes())
        );
    }

    fn trace_params_of(
        trace: Option<TraceParams<'_>>,
        headers: &HeaderMap,
    ) -> Vec<(Vec<u8>, Vec<u8>)> {
        build_params(
            "GET",
            "/orders/1042",
            headers,
            Path::new("/srv/www/shop"),
            None,
            None,
            false,
            "127.0.0.1:1".parse().unwrap(),
            "127.0.0.1:80".parse().unwrap(),
            None,
            trace,
        )
    }

    fn trace_names(pairs: &[(Vec<u8>, Vec<u8>)]) -> Vec<&[u8]> {
        pairs
            .iter()
            .map(|(k, _)| k.as_slice())
            .filter(|k| k.starts_with(b"YERD_TRACE_"))
            .collect()
    }

    #[test]
    fn trace_param_names_match_the_extension_contract() {
        assert_eq!(TRACE_ID_PARAM.as_bytes(), b"YERD_TRACE_ID");
        assert_eq!(TRACE_PORT_PARAM.as_bytes(), b"YERD_TRACE_PORT");
        assert_eq!(TRACE_SITE_PARAM.as_bytes(), b"YERD_TRACE_SITE");
        assert_eq!(TRACE_FEATURES_PARAM.as_bytes(), b"YERD_TRACE_FEATURES");
    }

    #[test]
    fn no_trace_emits_no_trace_params_and_keeps_the_list_unchanged() {
        let headers = make_headers("shop.test");
        let without = trace_params_of(None, &headers);
        assert!(trace_names(&without).is_empty());
        let traced = trace_params_of(
            Some(TraceParams {
                id: "0123456789abcdef0000000000000001",
                port: 9912,
                site: "shop",
                features: "dumps,queries",
            }),
            &headers,
        );
        assert_eq!(&traced[..without.len()], without.as_slice());
    }

    #[test]
    fn trace_appends_exactly_four_params() {
        let pairs = trace_params_of(
            Some(TraceParams {
                id: "0123456789abcdef0000000000000001",
                port: 9912,
                site: "shop",
                features: "",
            }),
            &make_headers("shop.test"),
        );
        let cases: [(&[u8], &[u8]); 4] = [
            (b"YERD_TRACE_ID", b"0123456789abcdef0000000000000001"),
            (b"YERD_TRACE_PORT", b"9912"),
            (b"YERD_TRACE_SITE", b"shop"),
            (b"YERD_TRACE_FEATURES", b""),
        ];
        assert_eq!(trace_names(&pairs).len(), cases.len());
        for (name, value) in cases {
            assert_eq!(lookup(&pairs, name), Some(value), "{name:?}");
        }
        let tail: Vec<&[u8]> = pairs[pairs.len() - 4..]
            .iter()
            .map(|(k, _)| k.as_slice())
            .collect();
        assert_eq!(tail, cases.map(|(name, _)| name).to_vec());
    }

    #[test]
    fn client_trace_header_cannot_forge_a_trace_param() {
        let mut headers = make_headers("shop.test");
        headers.insert("Yerd-Trace-Id", "forged".parse().unwrap());
        let pairs = trace_params_of(None, &headers);
        assert_eq!(
            lookup(&pairs, b"HTTP_YERD_TRACE_ID"),
            Some(b"forged".as_slice())
        );
        assert!(lookup(&pairs, b"YERD_TRACE_ID").is_none());
    }
}
