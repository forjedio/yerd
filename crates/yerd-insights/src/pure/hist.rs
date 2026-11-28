//! Fixed-bucket latency histogram.
//!
//! 64 geometric buckets in microseconds: bucket 0 holds everything under
//! 500 µs, bucket `i` has an upper bound of about `500 µs × 1.25^i`, and bucket
//! 63 (about 635 s) also takes everything above it. Quantiles report the
//! bucket's geometric midpoint, so the error is about ±12.5%.
//!
//! Bounds are computed by integer steps of `× 5 / 4` from 500, so they are
//! deterministic on every platform.

use serde::{Deserialize, Serialize};

use crate::pure::num::{f, round_u64};

/// Number of buckets.
pub const BUCKETS: u8 = 64;

/// Upper bound of bucket 0, in microseconds.
const BASE_US: u64 = 500;

/// A sparse latency histogram: `(bucket, count)` pairs sorted by bucket.
///
/// Serialises as `[[bucket, count], ...]`. Deserialising normalises the list,
/// so unsorted, duplicated or out-of-range buckets are tolerated.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "Vec<(u8, u32)>", into = "Vec<(u8, u32)>")]
pub struct LatencyHist {
    buckets: Vec<(u8, u32)>,
}

impl From<Vec<(u8, u32)>> for LatencyHist {
    fn from(raw: Vec<(u8, u32)>) -> Self {
        let mut h = Self::default();
        for (b, c) in raw {
            h.add(b.min(BUCKETS - 1), c);
        }
        h
    }
}

impl From<LatencyHist> for Vec<(u8, u32)> {
    fn from(h: LatencyHist) -> Self {
        h.buckets
    }
}

/// The exclusive upper bound of bucket `i` in microseconds.
#[must_use]
pub fn upper_bound(i: u8) -> u64 {
    let mut b = BASE_US;
    for _ in 0..i {
        b = b.saturating_mul(5) / 4;
    }
    b
}

/// The bucket a duration falls into.
#[must_use]
pub fn bucket_of(us: u64) -> u8 {
    let mut b = BASE_US;
    for i in 0..BUCKETS - 1 {
        if us < b {
            return i;
        }
        b = b.saturating_mul(5) / 4;
    }
    BUCKETS - 1
}

/// The representative value reported for bucket `i`: the geometric midpoint of
/// its bounds, or half of the first bound for bucket 0.
#[must_use]
pub fn midpoint(i: u8) -> u64 {
    if i == 0 {
        return BASE_US / 2;
    }
    let lo = f(upper_bound(i - 1));
    let hi = f(upper_bound(i));
    round_u64((lo * hi).sqrt())
}

impl LatencyHist {
    /// An empty histogram.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one duration in microseconds.
    pub fn record(&mut self, us: u64) {
        self.add(bucket_of(us), 1);
    }

    fn add(&mut self, bucket: u8, count: u32) {
        if count == 0 {
            return;
        }
        match self.buckets.binary_search_by_key(&bucket, |(b, _)| *b) {
            Ok(pos) => {
                if let Some((_, c)) = self.buckets.get_mut(pos) {
                    *c = c.saturating_add(count);
                }
            }
            Err(pos) => self.buckets.insert(pos, (bucket, count)),
        }
    }

    /// Add every count of `other` into `self`.
    pub fn merge(&mut self, other: &Self) {
        for &(b, c) in &other.buckets {
            self.add(b, c);
        }
    }

    /// Total recorded durations.
    #[must_use]
    pub fn count(&self) -> u64 {
        self.buckets.iter().map(|(_, c)| u64::from(*c)).sum()
    }

    /// True when nothing was recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.buckets.is_empty()
    }

    /// The `q` quantile (`0.0..=1.0`) in microseconds, or `None` when empty.
    #[must_use]
    pub fn quantile(&self, q: f64) -> Option<u64> {
        let total = self.count();
        if total == 0 {
            return None;
        }
        let q = if q.is_nan() { 0.0 } else { q.clamp(0.0, 1.0) };
        let rank = round_u64((q * f(total)).ceil()).clamp(1, total);
        let mut seen = 0u64;
        for &(b, c) in &self.buckets {
            seen += u64::from(c);
            if seen >= rank {
                return Some(midpoint(b));
            }
        }
        self.buckets.last().map(|(b, _)| midpoint(*b))
    }

    /// The sparse `(bucket, count)` pairs, sorted by bucket.
    #[must_use]
    pub fn buckets(&self) -> &[(u8, u32)] {
        &self.buckets
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_precision_loss
)]
mod tests {
    use super::*;

    #[test]
    fn bucket_boundaries() {
        let cases: &[(u64, u8)] = &[
            (0, 0),
            (499, 0),
            (500, 1),
            (624, 1),
            (625, 2),
            (781, 3),
            (u64::MAX, 63),
        ];
        for (us, want) in cases {
            assert_eq!(bucket_of(*us), *want, "{us}");
        }
        assert_eq!(upper_bound(0), 500);
        assert_eq!(upper_bound(1), 625);
        assert_eq!(upper_bound(2), 781);
        let top = upper_bound(63);
        assert!((600_000_000..700_000_000).contains(&top), "{top}");
    }

    #[test]
    fn clamps_above_the_last_bucket() {
        let mut h = LatencyHist::new();
        h.record(upper_bound(63) * 10);
        h.record(u64::MAX);
        assert_eq!(h.buckets(), &[(63, 2)]);
    }

    #[test]
    fn merge_is_associative_and_counts_add() {
        let mk = |xs: &[u64]| {
            let mut h = LatencyHist::new();
            for x in xs {
                h.record(*x);
            }
            h
        };
        let a = mk(&[100, 2_000, 50_000]);
        let b = mk(&[2_000, 9_000]);
        let c = mk(&[100, 1_000_000]);
        let mut ab_c = a.clone();
        ab_c.merge(&b);
        ab_c.merge(&c);
        let mut bc = b.clone();
        bc.merge(&c);
        let mut a_bc = a.clone();
        a_bc.merge(&bc);
        assert_eq!(ab_c, a_bc);
        assert_eq!(ab_c.count(), 7);
        assert_eq!(
            ab_c,
            mk(&[100, 2_000, 50_000, 2_000, 9_000, 100, 1_000_000])
        );
    }

    #[test]
    fn quantiles_on_known_distributions() {
        let mut h = LatencyHist::new();
        for i in 1..=100u64 {
            h.record(i * 1_000);
        }
        let p50 = h.quantile(0.5).unwrap() as f64;
        let p95 = h.quantile(0.95).unwrap() as f64;
        assert!((p50 - 50_000.0).abs() / 50_000.0 < 0.125, "{p50}");
        assert!((p95 - 95_000.0).abs() / 95_000.0 < 0.125, "{p95}");

        let mut single = LatencyHist::new();
        single.record(10_000);
        let v = single.quantile(0.5).unwrap();
        assert_eq!(single.quantile(0.0), Some(v));
        assert_eq!(single.quantile(1.0), Some(v));
        assert!((v as f64 - 10_000.0).abs() / 10_000.0 < 0.125, "{v}");
    }

    #[test]
    fn empty_quantile_is_none() {
        let h = LatencyHist::new();
        assert!(h.is_empty());
        assert_eq!(h.quantile(0.5), None);
        assert_eq!(h.count(), 0);
    }

    #[test]
    fn sparse_round_trip() {
        let mut h = LatencyHist::new();
        h.record(100);
        h.record(100);
        h.record(30_000);
        let s = serde_json::to_string(&h).unwrap();
        assert_eq!(s, format!("[[0,2],[{},1]]", bucket_of(30_000)));
        assert_eq!(serde_json::from_str::<LatencyHist>(&s).unwrap(), h);
        let messy: LatencyHist = serde_json::from_str("[[9,1],[2,1],[9,2],[200,1],[3,0]]").unwrap();
        assert_eq!(messy.buckets(), &[(2, 1), (9, 3), (63, 1)]);
    }
}
