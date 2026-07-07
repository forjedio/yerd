//! Small query-string helpers for the one-click `WordPress` login flow -
//! extracting the `yerd_login_token` value, and stripping it back out again
//! before the request is ever forwarded to PHP (see `dispatch`'s interception
//! branch in `server.rs`).

/// Find the first value of query param `name`, given a request's raw
/// `Uri::query()` (the part after `?`, no leading `?`). No percent-decoding -
/// this exists only for yerd's own plain-hex login token, not general query
/// strings.
pub fn get_param<'a>(query: Option<&'a str>, name: &str) -> Option<&'a str> {
    let query = query?;
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then_some(value)
    })
}

/// Remove every occurrence of query param `name` from `path_and_query` (a
/// full `"<path>?<query>"` or bare `"<path>"` string), preserving the order of
/// remaining params. Drops the `?` entirely if no params remain.
pub fn strip_param(path_and_query: &str, name: &str) -> String {
    let Some((path, query)) = path_and_query.split_once('?') else {
        return path_and_query.to_owned();
    };
    let kept: Vec<&str> = query
        .split('&')
        .filter(|pair| {
            let key = match pair.split_once('=') {
                Some((k, _)) => k,
                None => pair,
            };
            key != name
        })
        .collect();
    if kept.is_empty() {
        path.to_owned()
    } else {
        format!("{path}?{}", kept.join("&"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_param_finds_value() {
        assert_eq!(
            get_param(Some("a=1&yerd_login_token=abc&b=2"), "yerd_login_token"),
            Some("abc")
        );
    }

    #[test]
    fn get_param_none_when_absent() {
        assert_eq!(get_param(Some("a=1&b=2"), "yerd_login_token"), None);
    }

    #[test]
    fn get_param_none_when_no_query() {
        assert_eq!(get_param(None, "yerd_login_token"), None);
    }

    #[test]
    fn strip_param_removes_target_leaves_others_in_order() {
        assert_eq!(
            strip_param(
                "/wp-admin/?a=1&yerd_login_token=abc&b=2",
                "yerd_login_token"
            ),
            "/wp-admin/?a=1&b=2"
        );
    }

    #[test]
    fn strip_param_only_param_present_drops_question_mark() {
        assert_eq!(
            strip_param("/wp-admin/?yerd_login_token=abc", "yerd_login_token"),
            "/wp-admin/"
        );
    }

    #[test]
    fn strip_param_absent_is_a_no_op() {
        assert_eq!(
            strip_param("/wp-admin/?a=1", "yerd_login_token"),
            "/wp-admin/?a=1"
        );
    }

    #[test]
    fn strip_param_no_query_string_at_all() {
        assert_eq!(strip_param("/wp-admin/", "yerd_login_token"), "/wp-admin/");
    }

    #[test]
    fn strip_param_removes_all_occurrences() {
        assert_eq!(
            strip_param(
                "/wp-admin/?yerd_login_token=abc&x=1&yerd_login_token=def",
                "yerd_login_token"
            ),
            "/wp-admin/?x=1"
        );
    }

    #[test]
    fn strip_param_bare_key_with_no_equals_sign() {
        assert_eq!(
            strip_param("/wp-admin/?yerd_login_token", "yerd_login_token"),
            "/wp-admin/"
        );
        assert_eq!(
            strip_param("/wp-admin/?flag&a=1", "yerd_login_token"),
            "/wp-admin/?flag&a=1"
        );
    }
}
