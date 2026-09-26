//! Pure static-file routing: turn a request URL path into a safe relative path
//! under the served root, plus a `Content-Type` lookup by extension.
//!
//! This is the *pure* half of nginx/Caddy's `try_files $uri /index.php`: decide
//! whether a request *could* map to a static file (returning a traversal-safe
//! relative path) or whether it belongs to the PHP front controller (`None`).
//! The caller performs the filesystem existence/type check - that's the I/O
//! half, in `forward::static_file`.

use std::path::{Path, PathBuf};

/// Turn a request URL path into a safe relative path under the served root, or
/// `None` when the request should go to the front controller (`index.php`).
///
/// Returns `None` for the site root (`/`), any directory request (trailing
/// `/`), and any path that fails the traversal guard. Every returned segment is
/// percent-decoded and verified to be a single, real path component (no `.`,
/// `..`, empty, or embedded `/`/NUL after decoding), so `root.join(rel)` cannot
/// escape `root` by string manipulation alone. The caller still canonicalises
/// as defence-in-depth against symlinks.
#[must_use]
pub fn static_candidate(url_path: &str) -> Option<PathBuf> {
    let path = url_path.split('?').next().unwrap_or(url_path);
    if path.is_empty() || path.ends_with('/') {
        return None;
    }

    let rel = resolve_segments(path)?;
    if rel.as_os_str().is_empty() {
        return None;
    }
    Some(rel)
}

/// Turn a request URL path into a safe relative **directory** path under the
/// served root, for resolving a directory-index file (`index.html` /
/// `index.htm`) when no `index.php` is present there.
///
/// `None` for anything that isn't a directory request (no trailing `/`, and
/// not the bare root `/`), or that fails the traversal guard. Percent-decoding
/// and traversal rules match [`static_candidate`]; unlike it, this accepts a
/// trailing `/` and returns the directory itself (the root `/` decodes to an
/// empty relative path, i.e. `served_root` itself).
#[must_use]
pub fn directory_candidate(url_path: &str) -> Option<PathBuf> {
    let path = url_path.split('?').next().unwrap_or(url_path);
    if !path.starts_with('/') || (path != "/" && !path.ends_with('/')) {
        return None;
    }

    resolve_segments(path)
}

/// Percent-decode and traversal-guard every `/`-separated segment of `path`,
/// building the safe relative path. `None` if any segment is a malformed
/// escape, empty, `.`/`..`, or contains an embedded `/`, `\`, or NUL after
/// decoding. Shared by [`static_candidate`] and [`directory_candidate`] so
/// the traversal guard can't drift between the two; each caller applies its
/// own path-shape gate (root/trailing-slash rules) before calling this.
fn resolve_segments(path: &str) -> Option<PathBuf> {
    let mut rel = PathBuf::new();
    for raw in path.split('/') {
        if raw.is_empty() {
            continue;
        }
        let seg = percent_decode(raw)?;
        if seg.is_empty() || seg == "." || seg == ".." {
            return None;
        }
        if seg.bytes().any(|b| b == b'/' || b == b'\\' || b == 0) {
            return None;
        }
        rel.push(seg);
    }
    Some(rel)
}

/// Split a `PATH_INFO`-style URL path at its first PHP-source segment - the
/// non-greedy half of nginx's `fastcgi_split_path_info ^(.+?\.php)(/.*)$` -
/// into the candidate script path and the decoded `PATH_INFO` remainder
/// (always starting with `/`).
///
/// `None` when no segment with trailing path data is PHP source (a plain
/// `/foo.php` has no remainder and is not a split; `/foo.php/` has the
/// slash-only remainder `/`), or when the script half fails the
/// same percent-decoding/traversal guard as [`static_candidate`]. Remainder
/// segments are decoded with the same escapes rule but deliberately allow
/// `.`/`..` and empty segments - `PATH_INFO` is opaque data for the script,
/// not a filesystem path, so `/file.php/a//b/` yields `/a//b/` exactly as
/// nginx's regex captures it - while still refusing embedded `/`, `\`, and
/// NUL after decoding. The caller must still verify the script half is a
/// real, on-disk file before trusting the split.
#[must_use]
pub fn php_split_candidate(url_path: &str) -> Option<(PathBuf, String)> {
    let path = url_path.split('?').next().unwrap_or(url_path);
    let components: Vec<&str> = path.split('/').collect();

    let split_at = components
        .iter()
        .take(components.len().saturating_sub(1))
        .position(|raw| {
            !raw.is_empty() && percent_decode(raw).is_some_and(|seg| is_php_source(Path::new(&seg)))
        })?;

    let mut script = PathBuf::new();
    for raw in components
        .get(..=split_at)?
        .iter()
        .filter(|s| !s.is_empty())
    {
        let seg = percent_decode(raw)?;
        if seg.is_empty() || seg == "." || seg == ".." {
            return None;
        }
        if seg.bytes().any(|b| b == b'/' || b == b'\\' || b == 0) {
            return None;
        }
        script.push(seg);
    }

    let mut info = String::new();
    for raw in components.get(split_at + 1..)? {
        let seg = percent_decode(raw)?;
        if seg.bytes().any(|b| b == b'/' || b == b'\\' || b == 0) {
            return None;
        }
        info.push('/');
        info.push_str(&seg);
    }

    Some((script, info))
}

/// Whether `path` looks like PHP source - these must never be served as a static
/// file (it would leak source), so the front controller handles them instead.
#[must_use]
pub fn is_php_source(path: &Path) -> bool {
    matches!(
        ext_lower(path).as_deref(),
        Some("php" | "phtml" | "php3" | "php4" | "php5" | "php7" | "phps" | "pht")
    )
}

/// The `Content-Type` to serve a static file with, keyed on its extension.
/// Falls back to `application/octet-stream` for anything unrecognised.
#[must_use]
pub fn content_type_for(path: &Path) -> &'static str {
    match ext_lower(path).as_deref() {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js" | "mjs" | "cjs") => "text/javascript; charset=utf-8",
        Some("json" | "map") => "application/json",
        Some("webmanifest") => "application/manifest+json",
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("avif") => "image/avif",
        Some("bmp") => "image/bmp",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",
        Some("eot") => "application/vnd.ms-fontobject",
        Some("txt") => "text/plain; charset=utf-8",
        Some("xml") => "application/xml",
        Some("pdf") => "application/pdf",
        Some("wasm") => "application/wasm",
        Some("mp4") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mp3") => "audio/mpeg",
        Some("wav") => "audio/wav",
        Some("zip") => "application/zip",
        _ => "application/octet-stream",
    }
}

/// Lowercased file extension, if any.
fn ext_lower(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
}

/// Percent-decode one URL path segment. Returns `None` on a malformed escape
/// (`%` not followed by two hex digits) or non-UTF-8 result.
fn percent_decode(s: &str) -> Option<String> {
    let mut out: Vec<u8> = Vec::with_capacity(s.len());
    let mut bytes = s.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let hi = hex_val(bytes.next()?)?;
            let lo = hex_val(bytes.next()?)?;
            out.push(hi * 16 + lo);
        } else {
            out.push(b);
        }
    }
    String::from_utf8(out).ok()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
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

    #[test]
    fn root_and_directories_go_to_front_controller() {
        assert_eq!(static_candidate("/"), None);
        assert_eq!(static_candidate(""), None);
        assert_eq!(static_candidate("/foo/"), None);
        assert_eq!(static_candidate("/foo/bar/"), None);
    }

    #[test]
    fn plain_file_paths_resolve() {
        assert_eq!(
            static_candidate("/favicon.ico"),
            Some(PathBuf::from("favicon.ico"))
        );
        assert_eq!(
            static_candidate("/build/assets/app.css"),
            Some(PathBuf::from("build/assets/app.css"))
        );
    }

    #[test]
    fn query_string_is_ignored() {
        assert_eq!(
            static_candidate("/app.js?v=123"),
            Some(PathBuf::from("app.js"))
        );
    }

    #[test]
    fn percent_encoded_segments_decode() {
        assert_eq!(
            static_candidate("/my%20file.png"),
            Some(PathBuf::from("my file.png"))
        );
    }

    #[test]
    fn traversal_is_rejected() {
        assert_eq!(static_candidate("/../etc/passwd"), None);
        assert_eq!(static_candidate("/foo/../../bar"), None);
        assert_eq!(static_candidate("/."), None);
        assert_eq!(static_candidate("/%2e%2e/secret"), None);
        assert_eq!(static_candidate("/foo%2fbar"), None);
        assert_eq!(static_candidate("/foo%2"), None);
        assert_eq!(static_candidate("/foo%zz"), None);
    }

    #[test]
    fn directory_candidate_accepts_root_and_trailing_slashes() {
        assert_eq!(directory_candidate("/"), Some(PathBuf::new()));
        assert_eq!(directory_candidate("/foo/"), Some(PathBuf::from("foo")));
        assert_eq!(
            directory_candidate("/foo/bar/"),
            Some(PathBuf::from("foo/bar"))
        );
        assert_eq!(directory_candidate("/foo/?x=1"), Some(PathBuf::from("foo")));
    }

    #[test]
    fn directory_candidate_rejects_non_directory_paths() {
        assert_eq!(directory_candidate(""), None);
        assert_eq!(directory_candidate("/foo"), None);
        assert_eq!(directory_candidate("/foo/bar"), None);
    }

    #[test]
    fn directory_candidate_rejects_traversal() {
        assert_eq!(directory_candidate("/../"), None);
        assert_eq!(directory_candidate("/foo/../../bar/"), None);
        assert_eq!(directory_candidate("/%2e%2e/"), None);
        assert_eq!(directory_candidate("/foo%2fbar/"), None);
    }

    #[test]
    fn php_split_finds_first_php_segment() {
        assert_eq!(
            php_split_candidate("/theme/styles.php/moove/123/all"),
            Some((
                PathBuf::from("theme/styles.php"),
                "/moove/123/all".to_owned()
            ))
        );
        assert_eq!(
            php_split_candidate("/lib/javascript.php/1/lib/javascript-static.js"),
            Some((
                PathBuf::from("lib/javascript.php"),
                "/1/lib/javascript-static.js".to_owned()
            ))
        );
        assert_eq!(
            php_split_candidate("/a.php/b.php/c"),
            Some((PathBuf::from("a.php"), "/b.php/c".to_owned()))
        );
    }

    #[test]
    fn php_split_yields_slash_only_path_info_for_trailing_slash() {
        assert_eq!(
            php_split_candidate("/file.php/"),
            Some((PathBuf::from("file.php"), "/".to_owned()))
        );
    }

    #[test]
    fn php_split_preserves_repeated_slashes_in_path_info() {
        assert_eq!(
            php_split_candidate("/file.php/a//b"),
            Some((PathBuf::from("file.php"), "/a//b".to_owned()))
        );
        assert_eq!(
            php_split_candidate("/file.php//"),
            Some((PathBuf::from("file.php"), "//".to_owned()))
        );
        assert_eq!(
            php_split_candidate("//dir//file.php/x"),
            Some((PathBuf::from("dir/file.php"), "/x".to_owned()))
        );
    }

    #[test]
    fn php_split_keeps_trailing_slash_and_decodes() {
        assert_eq!(
            php_split_candidate("/file.php/dir/"),
            Some((PathBuf::from("file.php"), "/dir/".to_owned()))
        );
        assert_eq!(
            php_split_candidate("/file.php/my%20arg?x=1"),
            Some((PathBuf::from("file.php"), "/my arg".to_owned()))
        );
    }

    #[test]
    fn php_split_ignores_plain_and_non_php_paths() {
        assert_eq!(php_split_candidate("/wp-login.php"), None);
        assert_eq!(php_split_candidate("/assets/app.css"), None);
        assert_eq!(php_split_candidate("/foo/bar"), None);
        assert_eq!(php_split_candidate("/"), None);
    }

    #[test]
    fn php_split_rejects_traversal_in_script_half() {
        assert_eq!(php_split_candidate("/../evil.php/x"), None);
        assert_eq!(php_split_candidate("/%2e%2e/evil.php/x"), None);
        assert_eq!(php_split_candidate("/a%2fb.php/x"), None);
    }

    #[test]
    fn php_sources_are_flagged() {
        assert!(is_php_source(Path::new("index.php")));
        assert!(is_php_source(Path::new("legacy.PHTML")));
        assert!(!is_php_source(Path::new("favicon.ico")));
        assert!(!is_php_source(Path::new("app.js")));
    }

    #[test]
    fn content_types_cover_common_assets() {
        assert_eq!(
            content_type_for(Path::new("index.html")),
            "text/html; charset=utf-8"
        );
        assert_eq!(
            content_type_for(Path::new("index.htm")),
            "text/html; charset=utf-8"
        );
        assert_eq!(content_type_for(Path::new("favicon.ico")), "image/x-icon");
        assert_eq!(
            content_type_for(Path::new("app.css")),
            "text/css; charset=utf-8"
        );
        assert_eq!(
            content_type_for(Path::new("app.js")),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(content_type_for(Path::new("logo.SVG")), "image/svg+xml");
        assert_eq!(content_type_for(Path::new("font.woff2")), "font/woff2");
        assert_eq!(
            content_type_for(Path::new("data.bin")),
            "application/octet-stream"
        );
        assert_eq!(
            content_type_for(Path::new("noext")),
            "application/octet-stream"
        );
    }
}
