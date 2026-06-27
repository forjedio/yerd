//! Pure-Rust responder: maps `(name, qtype, configured_tld)` to an
//! [`crate::Answer`].
//!
//! No I/O, no async, no upstream-server types in the visible API. The crate's
//! only caller is `LoopbackHandler` in `server.rs`, which translates the wire
//! qtype into a [`QClass`] before invoking [`Responder::answer`].

use yerd_core::Tld;

use crate::answer::{Answer, QClass};

/// Pure authoritative responder for a single TLD.
pub struct Responder {
    tld: Tld,
}

impl Responder {
    /// Construct a responder that answers for `tld`.
    #[must_use]
    pub fn new(tld: Tld) -> Self {
        Self { tld }
    }

    /// Classify a query.
    ///
    /// `name` is the query owner name (with or without a trailing dot).
    /// `qtype` is the crate-internal classification; the caller in
    /// `server.rs` translates the wire qtype → [`QClass`].
    pub(crate) fn answer(&self, name: &str, qtype: QClass) -> Answer {
        let bytes = name.as_bytes();

        let bytes = match bytes.split_last() {
            Some((&b'.', rest)) => rest,
            _ => bytes,
        };

        let tld_bytes = self.tld.as_str().as_bytes();

        let is_apex = bytes.len() == tld_bytes.len() && bytes.eq_ignore_ascii_case(tld_bytes);
        let ends_in_dot_tld = bytes.len() > tld_bytes.len()
            && bytes.get(bytes.len() - tld_bytes.len() - 1) == Some(&b'.')
            && bytes
                .get(bytes.len() - tld_bytes.len()..)
                .is_some_and(|s| s.eq_ignore_ascii_case(tld_bytes));

        if !is_apex && !ends_in_dot_tld {
            return Answer::Refused;
        }

        if is_apex {
            return Answer::NoData;
        }

        if bytes.first() == Some(&b'.') {
            return Answer::NxDomain;
        }
        if bytes.windows(2).any(|w| w == b"..") {
            return Answer::NxDomain;
        }

        match qtype {
            QClass::A => Answer::Loopback4,
            QClass::Aaaa => Answer::Loopback6,
            QClass::Other => Answer::NoData,
        }
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

    fn r(tld: &str) -> Responder {
        Responder::new(Tld::new(tld).unwrap())
    }

    #[test]
    fn answer_table() {
        let cases: &[(&str, QClass, &str, Answer)] = &[
            ("app.test", QClass::A, "test", Answer::Loopback4),
            ("app.test", QClass::Aaaa, "test", Answer::Loopback6),
            ("app.test", QClass::Other, "test", Answer::NoData),
            ("app.test.", QClass::A, "test", Answer::Loopback4),
            ("APP.TEST", QClass::A, "test", Answer::Loopback4),
            ("a.b.c.app.test", QClass::A, "test", Answer::Loopback4),
            ("test", QClass::A, "test", Answer::NoData),
            ("test", QClass::Aaaa, "test", Answer::NoData),
            ("test", QClass::Other, "test", Answer::NoData),
            ("test.", QClass::A, "test", Answer::NoData),
            ("app.com", QClass::A, "test", Answer::Refused),
            ("app.testify", QClass::A, "test", Answer::Refused),
            ("testify", QClass::A, "test", Answer::Refused),
            ("xapp.test", QClass::A, "test", Answer::Loopback4),
            ("app.somethingtest", QClass::A, "test", Answer::Refused),
            ("", QClass::A, "test", Answer::Refused),
            (".", QClass::A, "test", Answer::Refused),
            (".test", QClass::A, "test", Answer::NxDomain),
            ("x..test", QClass::A, "test", Answer::NxDomain),
            ("app..test", QClass::A, "test", Answer::NxDomain),
            ("app.dev.local", QClass::A, "dev.local", Answer::Loopback4),
            ("dev.local", QClass::A, "dev.local", Answer::NoData),
            ("local", QClass::A, "dev.local", Answer::Refused),
            (".local", QClass::A, "dev.local", Answer::Refused),
            ("a.dev-local", QClass::A, "dev.local", Answer::Refused),
        ];

        for (idx, (name, qtype, tld, expected)) in cases.iter().enumerate() {
            let got = r(tld).answer(name, *qtype);
            assert_eq!(
                got, *expected,
                "row {idx} ({name:?} qtype={qtype:?} tld={tld:?}): expected {expected:?}, got {got:?}"
            );
        }
    }
}
