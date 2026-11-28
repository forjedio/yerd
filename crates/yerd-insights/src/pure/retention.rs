//! Retention: hour keys, hour-file names, site directory names and the prune
//! plan.
//!
//! Data lives in per-site, per-UTC-hour files named `<YYYYMMDDHH>.roll`,
//! `.req` and `.det`. Hour files are dropped once they ended more than 24h
//! ago, then the oldest hours across every site while the total exceeds one
//! global cap. These are the only disk bounds.

use std::collections::BTreeMap;

/// One hour in milliseconds.
pub const HOUR_MS: u64 = 3_600_000;

/// How long data is kept.
pub const RETENTION_MS: u64 = 24 * HOUR_MS;

/// The global disk cap across every site.
pub const GLOBAL_CAP_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Bytes appended since the last prune that trigger a cap check.
pub const PRUNE_CHECK_BYTES: u64 = 128 * 1024 * 1024;

/// Which file of an hour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FileKind {
    /// Rollup deltas and dictionary lines.
    Roll,
    /// Request summary lines.
    Req,
    /// Gzip detail members.
    Det,
}

impl FileKind {
    /// The file extension, without the dot.
    #[must_use]
    pub fn ext(self) -> &'static str {
        match self {
            Self::Roll => "roll",
            Self::Req => "req",
            Self::Det => "det",
        }
    }

    fn from_ext(ext: &str) -> Option<Self> {
        match ext {
            "roll" => Some(Self::Roll),
            "req" => Some(Self::Req),
            "det" => Some(Self::Det),
            _ => None,
        }
    }
}

/// The hour index (hours since the Unix epoch) containing `ms`.
#[must_use]
pub fn hour_of(ms: u64) -> u64 {
    ms / HOUR_MS
}

/// Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let m = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Days since 1970-01-01 from a civil date.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = i64::from(m);
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        _ => 28,
    }
}

/// The file name for an hour and kind: `<YYYYMMDDHH>.<ext>`, UTC.
#[must_use]
pub fn file_name(hour: u64, kind: FileKind) -> String {
    let days = i64::try_from(hour / 24).unwrap_or(i64::MAX / 2);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}{m:02}{d:02}{:02}.{}", hour % 24, kind.ext())
}

/// Parse a name produced by [`file_name`]. `None` for anything else.
#[must_use]
pub fn parse_file_name(name: &str) -> Option<(u64, FileKind)> {
    let (stem, ext) = name.split_once('.')?;
    let kind = FileKind::from_ext(ext)?;
    if stem.len() != 10 || !stem.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let y: i64 = stem.get(0..4)?.parse().ok()?;
    let m: u32 = stem.get(4..6)?.parse().ok()?;
    let d: u32 = stem.get(6..8)?.parse().ok()?;
    let h: u64 = stem.get(8..10)?.parse().ok()?;
    if !(1..=12).contains(&m) || d == 0 || d > days_in_month(y, m) || h > 23 || y < 1970 {
        return None;
    }
    let days = u64::try_from(days_from_civil(y, m, d)).ok()?;
    Some((days * 24 + h, kind))
}

/// Whether an hour's files are past retention at `now_ms`.
#[must_use]
pub fn hour_expired(hour: u64, now_ms: u64) -> bool {
    (hour + 1) * HOUR_MS + RETENTION_MS <= now_ms
}

/// The `(site, hour)` pairs to delete. `files` lists `(site, file name, len)`;
/// names that do not parse are ignored. Expired hours go first, then the
/// oldest hours across all sites until the total fits under `global_cap`.
/// The result is sorted.
#[must_use]
pub fn prune_plan(
    files: &[(String, String, u64)],
    now_ms: u64,
    global_cap: u64,
) -> Vec<(String, u64)> {
    let mut hours: BTreeMap<(u64, String), u64> = BTreeMap::new();
    for (site, name, len) in files {
        if let Some((hour, _)) = parse_file_name(name) {
            let e = hours.entry((hour, site.clone())).or_insert(0);
            *e = e.saturating_add(*len);
        }
    }
    let mut plan: Vec<(String, u64)> = Vec::new();
    let mut total: u64 = hours.values().copied().sum();
    for ((hour, site), len) in &hours {
        if hour_expired(*hour, now_ms) || total > global_cap {
            plan.push((site.clone(), *hour));
            total = total.saturating_sub(*len);
        }
    }
    plan.sort();
    plan
}

fn hex_digit(n: u8) -> char {
    char::from_digit(u32::from(n), 16).unwrap_or('0')
}

/// A filesystem-safe directory name for a site: `[a-z0-9._-]` are kept and
/// every other byte becomes `%xx`. A name that would be empty, `.` or `..` is
/// fully encoded.
#[must_use]
pub fn site_dir_name(site: &str) -> String {
    let keep =
        |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-');
    let encode_all = site.is_empty() || site == "." || site == "..";
    let mut out = String::with_capacity(site.len());
    for b in site.bytes() {
        if keep(b) && !encode_all {
            out.push(char::from(b));
        } else {
            out.push('%');
            out.push(hex_digit(b >> 4));
            out.push(hex_digit(b & 0xf));
        }
    }
    if out.is_empty() {
        out.push('%');
    }
    out
}

/// Reverse [`site_dir_name`]. `None` for a name it could not have produced.
#[must_use]
pub fn site_from_dir_name(dir: &str) -> Option<String> {
    if dir == "%" {
        return Some(String::new());
    }
    let mut out = Vec::with_capacity(dir.len());
    let mut bytes = dir.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let hi = char::from(bytes.next()?).to_digit(16)?;
            let lo = char::from(bytes.next()?).to_digit(16)?;
            out.push(u8::try_from(hi * 16 + lo).ok()?);
        } else if b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-') {
            out.push(b);
        } else {
            return None;
        }
    }
    let site = String::from_utf8(out).ok()?;
    (site_dir_name(&site) == dir).then_some(site)
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

    fn hour_at(y: i64, m: u32, d: u32, h: u64) -> u64 {
        u64::try_from(days_from_civil(y, m, d)).unwrap() * 24 + h
    }

    #[test]
    fn file_names_across_boundaries() {
        let cases: &[(u64, &str)] = &[
            (0, "1970010100.roll"),
            (hour_at(2023, 12, 31, 23), "2023123123.roll"),
            (hour_at(2024, 1, 1, 0), "2024010100.roll"),
            (hour_at(2024, 2, 29, 12), "2024022912.roll"),
            (hour_at(2024, 3, 1, 0), "2024030100.roll"),
            (hour_at(2100, 3, 1, 5), "2100030105.roll"),
        ];
        for (hour, want) in cases {
            assert_eq!(file_name(*hour, FileKind::Roll), *want);
            assert_eq!(parse_file_name(want), Some((*hour, FileKind::Roll)));
        }
        assert_eq!(hour_at(2023, 12, 31, 23) + 1, hour_at(2024, 1, 1, 0));
        assert_eq!(hour_of(1_700_000_000_000), 472_222);
        assert_eq!(file_name(472_222, FileKind::Req), "2023111422.req");
        assert_eq!(file_name(472_222, FileKind::Det), "2023111422.det");
    }

    #[test]
    fn rejects_foreign_names() {
        for name in [
            "2023111422.txt",
            "202311142.roll",
            "2023131422.roll",
            "2023022922.roll",
            "2023111424.roll",
            "dismissed.json",
            "abcdefghij.req",
            "2023111422",
        ] {
            assert_eq!(parse_file_name(name), None, "{name}");
        }
    }

    #[test]
    fn prune_by_age() {
        let now = 100 * HOUR_MS + 1;
        let files = vec![
            ("a".to_owned(), file_name(75, FileKind::Roll), 1),
            ("a".to_owned(), file_name(75, FileKind::Req), 1),
            ("a".to_owned(), file_name(76, FileKind::Roll), 1),
            ("b".to_owned(), file_name(74, FileKind::Det), 1),
            ("b".to_owned(), "dismissed.json".to_owned(), 1),
        ];
        assert_eq!(
            prune_plan(&files, now, GLOBAL_CAP_BYTES),
            vec![("a".to_owned(), 75), ("b".to_owned(), 74)]
        );
    }

    #[test]
    fn prune_by_global_cap_oldest_first_across_sites() {
        let now = 100 * HOUR_MS;
        let files = vec![
            ("a".to_owned(), file_name(97, FileKind::Det), 40),
            ("b".to_owned(), file_name(98, FileKind::Det), 40),
            ("a".to_owned(), file_name(99, FileKind::Det), 40),
            ("b".to_owned(), file_name(99, FileKind::Req), 5),
        ];
        assert_eq!(prune_plan(&files, now, 1_000), vec![]);
        assert_eq!(prune_plan(&files, now, 90), vec![("a".to_owned(), 97)]);
        assert_eq!(
            prune_plan(&files, now, 50),
            vec![("a".to_owned(), 97), ("b".to_owned(), 98)]
        );
        assert_eq!(
            prune_plan(&files, now, 40),
            vec![
                ("a".to_owned(), 97),
                ("a".to_owned(), 99),
                ("b".to_owned(), 98),
            ]
        );
    }

    #[test]
    fn site_dirs_are_safe() {
        let cases: &[(&str, &str)] = &[
            ("blog", "blog"),
            ("my-app_2.x", "my-app_2.x"),
            ("Blog", "%42log"),
            ("a/b", "a%2fb"),
            ("..", "%2e%2e"),
            (".", "%2e"),
            ("", "%"),
            ("é", "%c3%a9"),
        ];
        for (site, want) in cases {
            assert_eq!(site_dir_name(site), *want, "{site}");
            assert_eq!(site_from_dir_name(want).as_deref(), Some(*site), "{want}");
        }
        for bad in ["Blog", "a%2", "a%zz", "%2fx%", "a/b", "%61"] {
            assert_eq!(site_from_dir_name(bad), None, "{bad}");
        }
    }
}
