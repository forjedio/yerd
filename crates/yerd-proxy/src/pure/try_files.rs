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

    let mut rel = PathBuf::new();
    let mut segments = 0usize;
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
        segments += 1;
    }
    if segments == 0 {
        return None;
    }
    Some(rel)
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
    fn php_sources_are_flagged() {
        assert!(is_php_source(Path::new("index.php")));
        assert!(is_php_source(Path::new("legacy.PHTML")));
        assert!(!is_php_source(Path::new("favicon.ico")));
        assert!(!is_php_source(Path::new("app.js")));
    }

    #[test]
    fn content_types_cover_common_assets() {
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
