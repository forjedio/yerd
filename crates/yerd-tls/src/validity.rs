//! Clock-free validity window for cert generation.
//!
//! [`Validity`] is the only timestamp-shaped type in `yerd-tls`'s public
//! surface. Every cert-generating call takes one by value; the crate never
//! reads "now". Callers (the daemon) read the clock and construct a window.
//!
//! Range policy: [`Validity::new`] rejects `not_before > not_after` and
//! rejects timestamps with `year() > 9998`. The year cap reserves a one-year
//! gap below `time`'s representable ceiling (the type is bounded to ±9999
//! without the `large-dates` feature) so callers cannot accidentally emit
//! `99991231235959Z` `GeneralizedTime` — several trust stores treat that as
//! "no expiry" or refuse it outright. Pre-1950 timestamps are *not* rejected
//! (RFC 5280 §4.2.1.5 permits `GeneralizedTime` for them); the daemon's
//! policy module is the right layer for "no certs from before unix epoch"
//! or similar.

use time::OffsetDateTime;

use crate::error::{TlsError, ValidityErrorReason};

/// A NotBefore/NotAfter pair validated at construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Validity {
    not_before: OffsetDateTime,
    not_after: OffsetDateTime,
}

impl Validity {
    /// Build a [`Validity`] from explicit endpoints.
    ///
    /// Fails with [`TlsError::Validity`] if `not_before > not_after` or
    /// either endpoint has `year() > 9998`.
    pub fn new(not_before: OffsetDateTime, not_after: OffsetDateTime) -> Result<Self, TlsError> {
        if not_before > not_after {
            return Err(TlsError::Validity {
                reason: ValidityErrorReason::NotBeforeAfterNotAfter,
            });
        }
        if not_before.year() > 9998 || not_after.year() > 9998 {
            return Err(TlsError::Validity {
                reason: ValidityErrorReason::YearAbove9998,
            });
        }
        Ok(Self {
            not_before,
            not_after,
        })
    }

    /// The configured `NotBefore` timestamp.
    #[must_use]
    pub fn not_before(&self) -> OffsetDateTime {
        self.not_before
    }

    /// The configured `NotAfter` timestamp.
    #[must_use]
    pub fn not_after(&self) -> OffsetDateTime {
        self.not_after
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
    use time::{Date, Month, Time};

    use super::*;
    use crate::error::ValidityErrorReason;

    fn at(year: i32, month: Month, day: u8) -> OffsetDateTime {
        Date::from_calendar_date(year, month, day)
            .unwrap()
            .with_time(Time::from_hms(0, 0, 0).unwrap())
            .assume_utc()
    }

    #[test]
    fn new_accepts_normal_range() {
        let v = Validity::new(at(2026, Month::January, 1), at(2027, Month::January, 1)).unwrap();
        assert_eq!(v.not_before(), at(2026, Month::January, 1));
        assert_eq!(v.not_after(), at(2027, Month::January, 1));
    }

    #[test]
    fn new_accepts_equal_endpoints() {
        let t = at(2026, Month::January, 1);
        let v = Validity::new(t, t).unwrap();
        assert_eq!(v.not_before(), t);
        assert_eq!(v.not_after(), t);
    }

    #[test]
    fn new_rejects_reversed() {
        let err =
            Validity::new(at(2027, Month::January, 1), at(2026, Month::January, 1)).unwrap_err();
        match err {
            TlsError::Validity { reason } => {
                assert_eq!(reason, ValidityErrorReason::NotBeforeAfterNotAfter);
            }
            other => panic!("expected Validity error, got {other:?}"),
        }
    }

    #[test]
    fn new_rejects_year_above_9998_via_9999_input() {
        // 9999 is the smallest representable value > 9998 on time 0.3.36
        // without `large-dates`.
        let err =
            Validity::new(at(2026, Month::January, 1), at(9999, Month::January, 1)).unwrap_err();
        match err {
            TlsError::Validity { reason } => {
                assert_eq!(reason, ValidityErrorReason::YearAbove9998);
            }
            other => panic!("expected Validity error, got {other:?}"),
        }
    }

    #[test]
    fn new_accepts_2049_to_2050_cutover() {
        // UTCTime → GeneralizedTime boundary; rcgen handles the encoding
        // switch, our validation does not care.
        Validity::new(at(2049, Month::January, 1), at(2050, Month::January, 1)).unwrap();
    }

    #[test]
    fn new_accepts_pre_1950() {
        // Pre-1950 is RFC 5280-legal via GeneralizedTime; we do not reject it.
        Validity::new(at(1900, Month::January, 1), at(2050, Month::January, 1)).unwrap();
    }

    #[test]
    fn accessors_return_inputs() {
        let nb = at(2024, Month::June, 15);
        let na = at(2025, Month::June, 15);
        let v = Validity::new(nb, na).unwrap();
        assert_eq!(v.not_before(), nb);
        assert_eq!(v.not_after(), na);
    }
}
