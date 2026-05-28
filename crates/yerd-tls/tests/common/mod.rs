//! Shared helpers for integration tests.
//!
//! Test-only file; the allow-block matches the workspace test-exemption policy.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    dead_code
)]

use time::{Date, Month, OffsetDateTime, Time};
use yerd_tls::Validity;

/// Build an `OffsetDateTime` at the given calendar date, UTC midnight.
pub fn at(year: i32, month: Month, day: u8) -> OffsetDateTime {
    Date::from_calendar_date(year, month, day)
        .unwrap()
        .with_time(Time::from_hms(0, 0, 0).unwrap())
        .assume_utc()
}

/// A standard validity window covering 2026 → 2027.
pub fn standard_validity() -> Validity {
    Validity::new(at(2026, Month::January, 1), at(2027, Month::January, 1)).unwrap()
}
