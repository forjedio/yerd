//! Route keys: how requests are grouped.
//!
//! Precedence: the extension's Laravel route template, then a directly
//! executed script (non-front-controller sites), then the normalised URL path.

use serde::{Deserialize, Serialize};

use crate::pure::contract::ExtRoute;
use crate::pure::sql::fnv;

/// Reserved route id that capped per-minute route maps fold overflow into.
pub const OTHER_ROUTE: u64 = 0;

/// Most path segments kept by [`normalise_path`]; the rest become `/…`.
pub const MAX_SEGMENTS: usize = 12;

/// Most bytes kept by [`normalise_path`].
pub const MAX_PATH_BYTES: usize = 200;

/// How a route was identified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteKind {
    /// A framework route template.
    Template,
    /// A directly executed PHP script.
    Script,
    /// A normalised URL path.
    Path,
}

impl RouteKind {
    /// The wire equivalent.
    #[must_use]
    pub fn to_ipc(self) -> yerd_ipc::InsightsRouteKind {
        match self {
            Self::Template => yerd_ipc::InsightsRouteKind::Template,
            Self::Script => yerd_ipc::InsightsRouteKind::Script,
            Self::Path => yerd_ipc::InsightsRouteKind::Path,
        }
    }
}

/// A request's route identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteKey {
    /// HTTP method, uppercased.
    pub method: String,
    /// Display label (`/orders/{order}`).
    pub label: String,
    /// How it was identified.
    pub kind: RouteKind,
    /// Framework route name.
    pub name: Option<String>,
    /// Framework action.
    pub action: Option<String>,
}

impl RouteKey {
    /// Stable id: FNV-1a over `method|label`.
    #[must_use]
    pub fn id(&self) -> u64 {
        route_id(&self.method, &self.label)
    }
}

/// Stable route id for a method and label.
#[must_use]
pub fn route_id(method: &str, label: &str) -> u64 {
    let mut s = String::with_capacity(method.len() + label.len() + 1);
    s.push_str(method);
    s.push('|');
    s.push_str(label);
    fnv(s.as_bytes())
}

/// The route key for a request. `script` is the executed script relative to
/// the document root (`None` for front-controller requests).
#[must_use]
pub fn route_key(
    method: &str,
    path: &str,
    script: Option<&str>,
    ext_route: Option<&ExtRoute>,
) -> RouteKey {
    let method = method.to_ascii_uppercase();
    if let Some(route) = ext_route {
        if let Some(uri) = route.uri.as_deref().filter(|u| !u.is_empty()) {
            let label = if uri.starts_with('/') {
                uri.to_owned()
            } else {
                format!("/{uri}")
            };
            return RouteKey {
                method,
                label,
                kind: RouteKind::Template,
                name: route.name.clone(),
                action: route.action.clone(),
            };
        }
    }
    if let Some(rel) = script.map(|s| s.trim_start_matches('/')) {
        if !rel.is_empty() && rel != "index.php" {
            return RouteKey {
                method,
                label: format!("/{rel}"),
                kind: RouteKind::Script,
                name: None,
                action: None,
            };
        }
    }
    RouteKey {
        method,
        label: normalise_path(path),
        kind: RouteKind::Path,
        name: None,
        action: None,
    }
}

fn is_uuid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    let lens = [8usize, 4, 4, 4, 12];
    parts.len() == lens.len()
        && parts
            .iter()
            .zip(lens)
            .all(|(p, n)| p.len() == n && p.chars().all(|c| c.is_ascii_hexdigit()))
}

fn placeholder(seg: &str) -> Option<&'static str> {
    if seg.chars().all(|c| c.is_ascii_digit()) {
        Some("{id}")
    } else if is_uuid(seg) {
        Some("{uuid}")
    } else if seg.len() >= 16 && seg.chars().all(|c| c.is_ascii_hexdigit()) {
        Some("{hash}")
    } else {
        None
    }
}

/// Normalise a request path: drop the query string, collapse empty segments,
/// replace numeric, UUID and long-hex segments with placeholders, and cap the
/// segment count and byte length.
#[must_use]
pub fn normalise_path(path: &str) -> String {
    let path = path.split(['?', '#']).next().unwrap_or("");
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return "/".to_owned();
    }
    let mut out = String::new();
    for seg in segments.iter().take(MAX_SEGMENTS) {
        out.push('/');
        out.push_str(placeholder(seg).unwrap_or(seg));
    }
    if segments.len() > MAX_SEGMENTS {
        out.push_str("/…");
    }
    truncate_utf8(&mut out, MAX_PATH_BYTES);
    out
}

/// Truncate `s` to at most `max` bytes on a char boundary.
pub(crate) fn truncate_utf8(s: &mut String, max: usize) {
    if s.len() <= max {
        return;
    }
    let mut cut = max;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    s.truncate(cut);
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::type_complexity,
    clippy::format_collect
)]
mod tests {
    use super::*;

    #[test]
    fn normalises_paths() {
        let long: String = (0..20).map(|i| format!("/s{i}")).collect();
        let long_want: String = (0..12).map(|i| format!("/s{i}")).collect::<String>() + "/…";
        let cases: Vec<(&str, String)> = vec![
            ("/orders/1042", "/orders/{id}".into()),
            (
                "/u/123e4567-e89b-12d3-a456-426614174000/edit",
                "/u/{uuid}/edit".into(),
            ),
            ("/files/0123456789abcdef0123", "/files/{hash}".into()),
            ("/files/abcdef", "/files/abcdef".into()),
            ("/", "/".into()),
            ("", "/".into()),
            ("/search?q=1&page=2", "/search".into()),
            ("//a///b/", "/a/b".into()),
            (&long, long_want),
        ];
        for (input, want) in &cases {
            assert_eq!(normalise_path(input), *want, "{input}");
        }
        let wide = format!("/{}", "x".repeat(300));
        assert_eq!(normalise_path(&wide).len(), MAX_PATH_BYTES);
        let multibyte = format!("/{}", "é".repeat(150));
        let n = normalise_path(&multibyte);
        assert!(n.len() <= MAX_PATH_BYTES);
        assert!(n.is_char_boundary(n.len()));
    }

    #[test]
    fn precedence() {
        let tpl = ExtRoute {
            uri: Some("orders/{order}".into()),
            name: Some("orders.show".into()),
            action: Some("OrderController@show".into()),
        };
        let root = ExtRoute {
            uri: Some("/".into()),
            name: None,
            action: None,
        };
        let no_uri = ExtRoute {
            uri: None,
            name: Some("x".into()),
            action: None,
        };
        let cases: Vec<(&str, Option<&str>, Option<&ExtRoute>, &str, RouteKind)> = vec![
            (
                "/orders/1",
                None,
                Some(&tpl),
                "/orders/{order}",
                RouteKind::Template,
            ),
            (
                "/orders/1",
                Some("wp-login.php"),
                Some(&tpl),
                "/orders/{order}",
                RouteKind::Template,
            ),
            ("/", None, Some(&root), "/", RouteKind::Template),
            (
                "/wp-login.php",
                Some("wp-login.php"),
                None,
                "/wp-login.php",
                RouteKind::Script,
            ),
            (
                "/orders/7",
                Some("index.php"),
                None,
                "/orders/{id}",
                RouteKind::Path,
            ),
            (
                "/orders/7",
                None,
                Some(&no_uri),
                "/orders/{id}",
                RouteKind::Path,
            ),
        ];
        for (path, script, ext, label, kind) in cases {
            let k = route_key("get", path, script, ext);
            assert_eq!(k.method, "GET");
            assert_eq!(k.label, label, "{path}");
            assert_eq!(k.kind, kind, "{path}");
        }
        let k = route_key("GET", "/orders/1", None, Some(&tpl));
        assert_eq!(k.name.as_deref(), Some("orders.show"));
        assert_eq!(k.action.as_deref(), Some("OrderController@show"));
    }

    #[test]
    fn id_is_stable() {
        let k = route_key("GET", "/orders/1042", None, None);
        assert_eq!(k.id(), fnv(b"GET|/orders/{id}"));
        assert_eq!(k.id(), 0x08f8_b377_d9e5_b86f);
        assert_ne!(k.id(), route_key("POST", "/orders/1042", None, None).id());
    }
}
