//! Lossy numeric conversions in one place, so the casts are audited once.
//! Every count and duration here is far below 2^52, so `u64 -> f64` is exact
//! in practice; `f64 -> u64` saturates and maps NaN and negatives to 0.

/// `n` as `f64`.
#[allow(clippy::cast_precision_loss)]
pub(crate) fn f(n: u64) -> f64 {
    n as f64
}

/// `v` rounded to the nearest `u64`, saturating; NaN and negatives give 0.
#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
pub(crate) fn round_u64(v: f64) -> u64 {
    if v.is_nan() || v <= 0.0 {
        0
    } else {
        v.round() as u64
    }
}

/// Microseconds as milliseconds.
pub(crate) fn us_to_ms(us: u64) -> f64 {
    f(us) / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions() {
        assert_eq!(round_u64(f64::NAN), 0);
        assert_eq!(round_u64(-3.0), 0);
        assert_eq!(round_u64(2.5), 3);
        assert_eq!(round_u64(f64::INFINITY), u64::MAX);
        assert!((us_to_ms(1_500) - 1.5).abs() < f64::EPSILON);
    }
}
