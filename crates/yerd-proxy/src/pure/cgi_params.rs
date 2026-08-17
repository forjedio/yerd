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
//! `PATH_INFO` is always `<original path>` either way - WordPress and
//! Laravel both route on `REQUEST_URI`, not `PATH_INFO`, so leaving it as the
//! full original path (rather than splitting "extra path after the script",
//! full CGI/1.1 `PATH_INFO` semantics) keeps this a minimal, low-risk change
//! on top of already-pinned behavior.
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
use std::path::{Path, PathBuf};

/// The one-click `WordPress` login flow's per-request FastCGI overrides -
/// present only for the one request that already proved it holds a valid,
/// now-consumed login token (see `dispatch` in `server.rs`); absent on every
/// other request. Bundled into one struct (rather than two parameters that
/// must always travel together) so "a target user with no prepend script" is
/// unrepresentable.
#[derive(Debug, Clone, Copy)]
pub struct AutoLoginParams<'a> {
    /// Path to the `auto_prepend_file` bootstrap script.
    pub prepend_script: &'a Path,
    /// The `WordPress` login/username to sign in as, or `""` for no
    /// preference (the prepend script falls back to the earliest-created
    /// administrator).
    pub target_user: &'a str,
}

/// The `(SCRIPT_FILENAME, SCRIPT_NAME)` pair for a request.
///
/// The filename is a filesystem path and goes through
/// [`yerd_core::path_norm::php_path`]; the name is a URL path and keeps slash
/// form on every OS. `None` falls back to the root `index.php` policy.
fn script_target(document_root: &Path, script_rel: Option<&Path>) -> (PathBuf, String) {
    match script_rel {
        Some(rel) => (
            yerd_core::path_norm::php_path(&document_root.join(rel)),
            format!("/{}", rel.to_string_lossy().replace('\\', "/")),
        ),
        None => (
            yerd_core::path_norm::php_path(&document_root.join("index.php")),
            "/index.php".to_owned(),
        ),
    }
}

/// Build the CGI parameter pairs. `script_rel`, if given, is a real,
/// on-disk `.php` file's path relative to `document_root` (see the module
/// doc) - `None` falls back to the root `index.php` policy. `auto_login`, if
/// given, adds a `PHP_VALUE: auto_prepend_file=<path>` param plus a custom
/// `YERD_LOGIN_USER` param carrying the target username - see
/// [`AutoLoginParams`].
///
/// Every filesystem path handed to PHP goes through
/// [`yerd_core::path_norm::php_path`] first: `DOCUMENT_ROOT`,
/// `SCRIPT_FILENAME`, and the `auto_prepend_file` value. That strips a Windows
/// verbatim (`\\?\`) prefix, which PHP cannot open at all, and settles the
/// separator on the native form, which is what PHP reports for the same file.
/// Normalising the composed `SCRIPT_FILENAME` rather than only the root matters
/// because a route-rule target arrives as a configured `api/index.php` whose
/// interior slash a `join` preserves.
///
/// `SCRIPT_NAME` is deliberately excluded: it is a URL path, not a filesystem
/// path, and stays slash-form on every OS. This is all a no-op off Windows.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn build_params(
    method: &str,
    path_and_query: &str,
    headers: &http::HeaderMap,
    document_root: &Path,
    script_rel: Option<&Path>,
    https: bool,
    remote_addr: SocketAddr,
    server_addr: SocketAddr,
    auto_login: Option<AutoLoginParams<'_>>,
) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut out: Vec<(Vec<u8>, Vec<u8>)> = Vec::with_capacity(16 + headers.len());

    let document_root = &yerd_core::path_norm::php_path(document_root);

    let (path, query) = split_path_query(path_and_query);
    let (script_filename, script_name) = script_target(document_root, script_rel);

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
    push(&mut out, b"PATH_INFO", path.as_bytes());
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
    if let Some(login) = auto_login {
        push(
            &mut out,
            b"PHP_VALUE",
            format!(
                "auto_prepend_file={}",
                yerd_core::path_norm::php_path(login.prepend_script).display()
            )
            .as_bytes(),
        );
        push(&mut out, b"YERD_LOGIN_USER", login.target_user.as_bytes());
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

    out
}

fn push(out: &mut Vec<(Vec<u8>, Vec<u8>)>, name: &[u8], value: &[u8]) {
    out.push((name.to_vec(), value.to_vec()));
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
    #[cfg(any(unix, windows))]
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

    /// Fixture site roots in the host's own form, so the byte-exact assertions
    /// below run on every OS instead of only Unix.
    #[cfg(windows)]
    const APP_ROOT: &str = r"C:\srv\www\app";
    #[cfg(not(windows))]
    const APP_ROOT: &str = "/srv/www/app";
    #[cfg(windows)]
    const BLOG_ROOT: &str = r"C:\srv\www\blog";
    #[cfg(not(windows))]
    const BLOG_ROOT: &str = "/srv/www/blog";

    /// `APP_ROOT` with `sep` appended, for composing an expected child path.
    fn under(root: &str, rel: &str) -> String {
        let sep = if cfg!(windows) { '\\' } else { '/' };
        format!("{root}{sep}{}", rel.replace('/', &sep.to_string()))
    }

    #[test]
    fn caddy_style_everything_to_index_php() {
        let root = PathBuf::from(APP_ROOT);
        let pairs = build_params(
            "GET",
            "/foo/bar?a=1&b=2",
            &make_headers("app.test"),
            &root,
            None,
            false,
            "127.0.0.1:54321".parse().unwrap(),
            "127.0.0.1:80".parse().unwrap(),
            None,
        );
        assert_eq!(
            lookup(&pairs, b"SCRIPT_NAME"),
            Some(b"/index.php".as_slice())
        );
        assert_eq!(
            lookup(&pairs, b"SCRIPT_FILENAME"),
            Some(under(APP_ROOT, "index.php").as_bytes())
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
        assert_eq!(lookup(&pairs, b"DOCUMENT_ROOT"), Some(APP_ROOT.as_bytes()));
        assert!(lookup(&pairs, b"HTTPS").is_none());
    }

    /// On Windows `document_root.join(script_rel)` yields native `\`
    /// separators, which PHP accepts for SCRIPT_FILENAME/DOCUMENT_ROOT; but
    /// SCRIPT_NAME is a URL path and must be normalized to `/` (the `:83`
    /// normalization - the classic separator bug). Pins the native filename
    /// form and the slashed script name together.
    #[cfg(windows)]
    #[test]
    fn windows_script_filename_is_native_but_script_name_is_slashed() {
        let root = PathBuf::from(r"C:\sites\shop");
        let rel = Path::new("wp-admin").join("index.php");
        let pairs = build_params(
            "GET",
            "/wp-admin/?page=1",
            &make_headers("shop.test"),
            &root,
            Some(rel.as_path()),
            false,
            "127.0.0.1:1".parse().unwrap(),
            "127.0.0.1:80".parse().unwrap(),
            None,
        );
        assert_eq!(
            lookup(&pairs, b"SCRIPT_FILENAME"),
            Some(r"C:\sites\shop\wp-admin\index.php".as_bytes())
        );
        assert_eq!(
            lookup(&pairs, b"DOCUMENT_ROOT"),
            Some(r"C:\sites\shop".as_bytes())
        );
        let script_name = lookup(&pairs, b"SCRIPT_NAME").unwrap();
        assert_eq!(script_name, b"/wp-admin/index.php".as_slice());
        assert!(
            !script_name.contains(&b'\\'),
            "SCRIPT_NAME must not contain backslashes"
        );
    }

    /// A verbatim (`\\?\`) document root - what `fs::canonicalize` returns on
    /// Windows, and what older builds persisted into `yerd.toml` - is stripped
    /// before it reaches PHP. Verified against a real `php-cgi.exe` 8.5: the
    /// verbatim form answers `404 No input file specified.` while the plain form
    /// serves the script, so this normalisation is what makes an existing
    /// Windows config work without rewriting it.
    #[cfg(windows)]
    #[test]
    fn windows_verbatim_document_root_is_stripped_before_php_sees_it() {
        let root = PathBuf::from(r"\\?\C:\sites\shop");
        let pairs = build_params(
            "GET",
            "/",
            &make_headers("shop.test"),
            &root,
            None,
            false,
            "127.0.0.1:1".parse().unwrap(),
            "127.0.0.1:80".parse().unwrap(),
            None,
        );
        assert_eq!(
            lookup(&pairs, b"SCRIPT_FILENAME"),
            Some(r"C:\sites\shop\index.php".as_bytes())
        );
        assert_eq!(
            lookup(&pairs, b"DOCUMENT_ROOT"),
            Some(r"C:\sites\shop".as_bytes())
        );
    }

    /// SCRIPT_NAME is always a URL path: on every OS a multi-segment
    /// `script_rel` built with the native separator must render with `/` only
    /// and never a `\`.
    #[test]
    fn script_name_uses_forward_slashes_on_every_os() {
        let rel = Path::new("wp-admin").join("index.php");
        let pairs = build_params(
            "GET",
            "/wp-admin/",
            &make_headers("blog.test"),
            Path::new(BLOG_ROOT),
            Some(rel.as_path()),
            false,
            "127.0.0.1:1".parse().unwrap(),
            "127.0.0.1:80".parse().unwrap(),
            None,
        );
        let script_name = lookup(&pairs, b"SCRIPT_NAME").unwrap();
        assert_eq!(script_name, b"/wp-admin/index.php".as_slice());
        assert!(!script_name.contains(&b'\\'));
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
            false,
            "127.0.0.1:1".parse().unwrap(),
            "127.0.0.1:80".parse().unwrap(),
            None,
        );
        let software = String::from_utf8_lossy(lookup(&pairs, b"SERVER_SOFTWARE").unwrap());
        assert!(software.contains("nginx"), "got {software:?}");
    }

    #[test]
    fn web_root_subdir_drives_script_filename_and_document_root() {
        let mut site =
            yerd_core::Site::linked("app", APP_ROOT, yerd_core::PhpVersion::new(8, 3)).unwrap();
        site.set_web_subpath("public");
        let served = site.served_root();
        let pairs = build_params(
            "GET",
            "/login",
            &make_headers("app.test"),
            &served,
            None,
            false,
            "127.0.0.1:1".parse().unwrap(),
            "127.0.0.1:80".parse().unwrap(),
            None,
        );
        assert_eq!(
            lookup(&pairs, b"DOCUMENT_ROOT"),
            Some(under(APP_ROOT, "public").as_bytes())
        );
        assert_eq!(
            lookup(&pairs, b"SCRIPT_FILENAME"),
            Some(under(APP_ROOT, "public/index.php").as_bytes())
        );
    }

    #[test]
    fn https_param_is_on_when_secure() {
        let pairs = build_params(
            "POST",
            "/",
            &make_headers("app.test"),
            Path::new(APP_ROOT),
            None,
            true,
            "1.2.3.4:1000".parse().unwrap(),
            "127.0.0.1:443".parse().unwrap(),
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
            false,
            "127.0.0.1:1".parse().unwrap(),
            "127.0.0.1:80".parse().unwrap(),
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
            false,
            "127.0.0.1:1".parse().unwrap(),
            "127.0.0.1:80".parse().unwrap(),
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
            false,
            "127.0.0.1:1".parse().unwrap(),
            "127.0.0.1:80".parse().unwrap(),
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
            Path::new(BLOG_ROOT),
            Some(Path::new("wp-admin/index.php")),
            false,
            "127.0.0.1:1".parse().unwrap(),
            "127.0.0.1:80".parse().unwrap(),
            None,
        );
        assert_eq!(
            lookup(&pairs, b"SCRIPT_NAME"),
            Some(b"/wp-admin/index.php".as_slice())
        );
        assert_eq!(
            lookup(&pairs, b"SCRIPT_FILENAME"),
            Some(under(BLOG_ROOT, "wp-admin/index.php").as_bytes())
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
            Path::new(BLOG_ROOT),
            None,
            false,
            "127.0.0.1:1".parse().unwrap(),
            "127.0.0.1:80".parse().unwrap(),
            Some(AutoLoginParams {
                prepend_script: Path::new("/data/wordpress-autologin-prepend.php"),
                target_user: "admin",
            }),
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
            Path::new(BLOG_ROOT),
            None,
            false,
            "127.0.0.1:1".parse().unwrap(),
            "127.0.0.1:80".parse().unwrap(),
            Some(AutoLoginParams {
                prepend_script: Path::new("/data/wordpress-autologin-prepend.php"),
                target_user: "",
            }),
        );
        assert_eq!(lookup(&pairs, b"YERD_LOGIN_USER"), Some(b"".as_slice()));
    }

    #[test]
    fn no_auto_login_omits_prepend_and_target_user_params() {
        let pairs = build_params(
            "GET",
            "/",
            &make_headers("app.test"),
            Path::new(APP_ROOT),
            None,
            false,
            "127.0.0.1:1".parse().unwrap(),
            "127.0.0.1:80".parse().unwrap(),
            None,
        );
        assert!(lookup(&pairs, b"PHP_VALUE").is_none());
        assert!(lookup(&pairs, b"YERD_LOGIN_USER").is_none());
    }

    #[test]
    fn resolved_exact_script_match_drives_script_name_and_filename() {
        let pairs = build_params(
            "POST",
            "/wp-login.php",
            &make_headers("blog.test"),
            Path::new(BLOG_ROOT),
            Some(Path::new("wp-login.php")),
            false,
            "127.0.0.1:1".parse().unwrap(),
            "127.0.0.1:80".parse().unwrap(),
            None,
        );
        assert_eq!(
            lookup(&pairs, b"SCRIPT_NAME"),
            Some(b"/wp-login.php".as_slice())
        );
        assert_eq!(
            lookup(&pairs, b"SCRIPT_FILENAME"),
            Some(under(BLOG_ROOT, "wp-login.php").as_bytes())
        );
    }

    /// The relational invariants frameworks actually depend on, across every
    /// root shape and both ways a script-relative path is produced.
    ///
    /// `WordPress`'s `get_home_path()` derives the install directory by
    /// comparing `SCRIPT_FILENAME` against `DOCUMENT_ROOT`, so the prefix
    /// relation must hold byte-for-byte. A `join`-built rel carries the native
    /// separator already; a config-built rel like `api/index.php` carries a
    /// forward slash inside one component, which is the case that produced a
    /// mixed form before the paths were normalised.
    #[test]
    fn script_filename_is_always_prefixed_by_document_root() {
        let roots = [
            "/srv/www/app",
            r"C:\srv\www\app",
            "C:/srv/www/app",
            r"\\server\share\app",
            r"\\?\C:\srv\www\app",
        ];
        let rels = [
            None,
            Some(PathBuf::from("api").join("index.php")),
            Some(PathBuf::from("api/index.php")),
        ];
        for root in roots {
            for rel in &rels {
                let pairs = build_params(
                    "GET",
                    "/api/thing",
                    &make_headers("app.test"),
                    Path::new(root),
                    rel.as_deref(),
                    false,
                    "127.0.0.1:9000".parse().unwrap(),
                    "127.0.0.1:80".parse().unwrap(),
                    None,
                );
                let doc_root = lookup(&pairs, b"DOCUMENT_ROOT").unwrap();
                let script = lookup(&pairs, b"SCRIPT_FILENAME").unwrap();
                let name = lookup(&pairs, b"SCRIPT_NAME").unwrap();

                assert!(
                    script.starts_with(doc_root),
                    "{root:?} + {rel:?}: SCRIPT_FILENAME {:?} must start with DOCUMENT_ROOT {:?}",
                    String::from_utf8_lossy(script),
                    String::from_utf8_lossy(doc_root)
                );
                assert!(
                    !name.contains(&b'\\'),
                    "{root:?} + {rel:?}: SCRIPT_NAME is a URL and must not contain a backslash"
                );
                if cfg!(windows) {
                    assert!(
                        !doc_root.contains(&b'/') && !script.contains(&b'/'),
                        "{root:?} + {rel:?}: filesystem paths must be all-backslash on Windows"
                    );
                }
            }
        }
    }
}
