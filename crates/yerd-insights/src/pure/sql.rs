//! SQL fingerprinting, hashing and read classification.
//!
//! A fingerprint is the query's shape: literals become `?`, `IN (...)` lists of
//! literals collapse to `IN (?+)`, whitespace collapses, and bare words are
//! lowercased. Quoted identifiers (`"col"`, `` `col` ``) are kept verbatim.
//! Laravel quotes every identifier it generates, so the bare words lowercased
//! here are effectively the keywords.

/// FNV-1a 64-bit offset basis.
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
/// FNV-1a 64-bit prime.
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// FNV-1a 64-bit hash.
#[must_use]
pub fn fnv(bytes: &[u8]) -> u64 {
    let mut h = FNV_OFFSET;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// Whether `next` is the sign of a decimal exponent ending `token` (`1e-3`).
/// Hex literals are excluded, so `0x1e-1` stays a subtraction.
fn signed_exponent(token: &str, next: char) -> bool {
    (next == '-' || next == '+')
        && token.ends_with(['e', 'E'])
        && !token.starts_with("0x")
        && !token.starts_with("0X")
}

/// The fingerprint of `sql`. See the module docs.
#[must_use]
pub fn fingerprint(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    let mut chars = sql.chars().peekable();
    let mut pending_space = false;
    let push = |out: &mut String, pending_space: &mut bool, s: &str| {
        if *pending_space && !out.is_empty() {
            out.push(' ');
        }
        *pending_space = false;
        out.push_str(s);
    };
    while let Some(c) = chars.next() {
        if c.is_whitespace() {
            pending_space = true;
            continue;
        }
        if c == '\'' {
            while let Some(n) = chars.next() {
                if n == '\\' {
                    chars.next();
                } else if n == '\'' {
                    if chars.peek() == Some(&'\'') {
                        chars.next();
                    } else {
                        break;
                    }
                }
            }
            push(&mut out, &mut pending_space, "?");
            continue;
        }
        if c == '"' || c == '`' {
            let mut quoted = String::new();
            quoted.push(c);
            for n in chars.by_ref() {
                quoted.push(n);
                if n == c {
                    break;
                }
            }
            push(&mut out, &mut pending_space, &quoted);
            continue;
        }
        if c.is_ascii_digit() {
            let glued = !pending_space && out.chars().next_back().is_some_and(is_word_char);
            let mut token = String::new();
            token.push(c);
            while let Some(&n) = chars.peek() {
                if n.is_alphanumeric() || n == '.' || n == '_' || signed_exponent(&token, n) {
                    token.push(n);
                    chars.next();
                } else {
                    break;
                }
            }
            if glued {
                push(&mut out, &mut pending_space, &token.to_lowercase());
            } else {
                push(&mut out, &mut pending_space, "?");
            }
            continue;
        }
        if c == '$' && chars.peek().is_some_and(char::is_ascii_digit) {
            while chars.peek().is_some_and(char::is_ascii_digit) {
                chars.next();
            }
            push(&mut out, &mut pending_space, "?");
            continue;
        }
        if is_word_char(c) {
            let mut word = String::new();
            word.push(c);
            while let Some(&n) = chars.peek() {
                if is_word_char(n) {
                    word.push(n);
                    chars.next();
                } else {
                    break;
                }
            }
            push(&mut out, &mut pending_space, &word.to_lowercase());
            continue;
        }
        let mut buf = [0u8; 4];
        push(&mut out, &mut pending_space, c.encode_utf8(&mut buf));
    }
    collapse_in_lists(&out)
}

/// Rewrite `in (?, ?, ...)` and `in (?)` to `in (?+)`.
fn collapse_in_lists(fp: &str) -> String {
    let mut out = String::with_capacity(fp.len());
    let mut rest = fp;
    while let Some(pos) = find_in_list(rest) {
        let (head, tail) = rest.split_at(pos);
        out.push_str(head);
        let Some(open) = tail.find('(') else {
            break;
        };
        let Some(close_rel) = tail.get(open..).and_then(|t| t.find(')')) else {
            out.push_str(tail);
            return out;
        };
        let close = open + close_rel;
        let inner = tail.get(open + 1..close).unwrap_or("");
        let is_list =
            !inner.trim().is_empty() && inner.chars().all(|c| c == '?' || c == ',' || c == ' ');
        if is_list {
            out.push_str(tail.get(..open).unwrap_or(""));
            out.push_str("(?+)");
            rest = tail.get(close + 1..).unwrap_or("");
        } else {
            out.push_str(tail.get(..=open).unwrap_or(""));
            rest = tail.get(open + 1..).unwrap_or("");
        }
    }
    out.push_str(rest);
    out
}

/// Byte offset of the next `in (` / `in(` keyword that starts a word,
/// outside quoted identifiers. `s` must start outside a quoted identifier.
fn find_in_list(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut at = 0;
    while let Some(&b) = bytes.get(at) {
        if b == b'"' || b == b'`' {
            at = bytes
                .get(at + 1..)
                .and_then(|t| t.iter().position(|n| *n == b))
                .map_or(bytes.len(), |rel| at + rel + 2);
            continue;
        }
        let before_ok = at == 0
            || bytes
                .get(at - 1)
                .is_some_and(|p| !(p.is_ascii_alphanumeric() || *p == b'_' || *p == b'$'));
        let after = s.get(at + 2..).unwrap_or("");
        let after_ok = after.starts_with(" (") || after.starts_with('(');
        if before_ok && after_ok && s.get(at..at + 2) == Some("in") {
            return Some(at);
        }
        at += 1;
    }
    None
}

/// `sql` with leading whitespace, `(` and comments (`/* */`, `--`, `#`)
/// removed.
fn skip_leading_noise(mut sql: &str) -> &str {
    loop {
        sql = sql.trim_start_matches(|c: char| c.is_whitespace() || c == '(');
        if let Some(rest) = sql.strip_prefix("/*") {
            sql = rest
                .find("*/")
                .map_or("", |end| rest.get(end + 2..).unwrap_or(""));
        } else if let Some(rest) = sql.strip_prefix("--").or_else(|| sql.strip_prefix('#')) {
            sql = rest
                .find('\n')
                .map_or("", |end| rest.get(end + 1..).unwrap_or(""));
        } else {
            return sql;
        }
    }
}

/// The first SQL keyword, uppercased, skipping whitespace, `(` and comments.
fn first_keyword(sql: &str) -> String {
    skip_leading_noise(sql)
        .chars()
        .take_while(char::is_ascii_alphabetic)
        .collect::<String>()
        .to_ascii_uppercase()
}

/// True when the statement reads (`SELECT`, `WITH` or `SHOW`).
#[must_use]
pub fn is_read(sql: &str) -> bool {
    matches!(first_keyword(sql).as_str(), "SELECT" | "WITH" | "SHOW")
}

/// True when a fingerprint contains a `limit` keyword outside quoted
/// identifiers.
#[must_use]
pub fn has_limit(fp: &str) -> bool {
    let mut quote: Option<char> = None;
    let mut word = String::new();
    for c in fp.chars().chain(std::iter::once(' ')) {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            continue;
        }
        if is_word_char(c) {
            word.push(c);
            continue;
        }
        if word.eq_ignore_ascii_case("limit") {
            return true;
        }
        word.clear();
        if c == '"' || c == '`' {
            quote = Some(c);
        }
    }
    false
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
    fn fingerprints() {
        let cases: &[(&str, &str)] = &[
            (
                "select * from `users` where `id` = 5",
                "select * from `users` where `id` = ?",
            ),
            (
                "SELECT * FROM users WHERE name = 'O''Brien' AND x = 'a\\'b'",
                "select * from users where name = ? and x = ?",
            ),
            (
                "select * from \"t1\" where \"col2\" = 3.5 and t2.c3 > -10",
                "select * from \"t1\" where \"col2\" = ? and t2.c3 > -?",
            ),
            (
                "select * from users where id in (1)",
                "select * from users where id in (?+)",
            ),
            (
                &format!(
                    "select * from users where id in ({})",
                    (1..=50)
                        .map(|i| i.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                "select * from users where id in (?+)",
            ),
            (
                "select * from users where id in (?, ?, ?)",
                "select * from users where id in (?+)",
            ),
            (
                "select * from users where id = $1 and b = $12",
                "select * from users where id = ? and b = ?",
            ),
            (
                "select * from users where id = ?",
                "select * from users where id = ?",
            ),
            (
                "SELECT *\n  FROM users\n\tWHERE id = 1\n",
                "select * from users where id = ?",
            ),
            (
                "select * from users where id in (select user_id from t)",
                "select * from users where id in (select user_id from t)",
            ),
            (
                "select * from `login` where `pin` = 1",
                "select * from `login` where `pin` = ?",
            ),
            ("select 0x1F", "select ?"),
            ("select 1e-3, 2E+10, 5 - 1", "select ?, ?, ? - ?"),
            ("select 0x1e-1", "select ?-?"),
            (
                "select * from u where id in (select id from v where k in (1, 2))",
                "select * from u where id in (select id from v where k in (?+))",
            ),
            (
                "select * from u where id in (select id from v where k in (1))",
                "select * from u where id in (select id from v where k in (?+))",
            ),
            (
                "select \"in (?)\", `a in (?, ?)` from t where x in (3)",
                "select \"in (?)\", `a in (?, ?)` from t where x in (?+)",
            ),
        ];
        for (sql, want) in cases {
            assert_eq!(fingerprint(sql), *want, "{sql}");
        }
    }

    #[test]
    fn hashes_are_stable() {
        assert_eq!(fnv(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(
            fnv(fingerprint("select * from users where id = 1").as_bytes()),
            fnv(b"select * from users where id = ?")
        );
    }

    #[test]
    fn read_classification() {
        let cases: &[(&str, bool)] = &[
            ("select 1", true),
            ("  (SELECT 1) union (select 2)", true),
            ("WITH x AS (select 1) select * from x", true),
            ("show tables", true),
            ("insert into t values (1)", false),
            ("update t set a = 1", false),
            ("", false),
            ("/* report */ SELECT * FROM t", true),
            ("-- tag\n  /* a */ (select 1)", true),
            ("# note\nshow tables", true),
            ("/* select */ delete from t", false),
            ("/* unterminated select", false),
        ];
        for (sql, want) in cases {
            assert_eq!(is_read(sql), *want, "{sql}");
        }
    }

    #[test]
    fn limit_detection() {
        let cases: &[(&str, bool)] = &[
            ("select * from t limit ?", true),
            ("select * from t limit ? offset ?", true),
            ("select * from t", false),
            ("select * from `limits`", false),
            ("select \"limit\" from t", false),
            ("select `limit` from t limit ?", true),
            ("select * from \"t\" limit ?", true),
        ];
        for (fp, want) in cases {
            assert_eq!(has_limit(fp), *want, "{fp}");
        }
    }
}
