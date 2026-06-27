//! The responder's output vocabulary plus crate-internal query classification.

/// What the responder decided for a single DNS query.
///
/// Wire-level encoding lives in `server.rs`; this enum is the pure
/// crate's output type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Answer {
    /// Matched, qtype = A → 127.0.0.1 with TTL [`crate::ANSWER_TTL_SECS`].
    Loopback4,
    /// Matched, qtype = AAAA → `::1` with TTL [`crate::ANSWER_TTL_SECS`].
    Loopback6,
    /// Name belongs to the configured TLD but the qtype is not A/AAAA.
    /// Wire: NOERROR with empty answer + no SOA in authority.
    NoData,
    /// Name is *within* the configured TLD but does not exist (e.g. an empty or
    /// malformed label). Wire: authoritative NXDOMAIN + no SOA.
    NxDomain,
    /// Name is *outside* the configured TLD - we are not authoritative for it.
    /// Wire: REFUSED with the AA bit cleared, so a resolver that (mis)routes a
    /// non-`.test` query here treats it as "ask someone else" rather than
    /// trusting an authoritative answer for a domain we don't own.
    Refused,
}

/// Crate-internal classification of a query type.
///
/// Lives here, not in the upstream server's record-type enum, so
/// `responder.rs` stays free of the upstream dependency. `server.rs` is
/// the only file that translates the wire qtype → `QClass`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QClass {
    A,
    Aaaa,
    /// MX, TXT, SOA, NS, ANY (per RFC 8482 §4.3), …
    Other,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answer_match_is_exhaustive() {
        match Answer::Loopback4 {
            Answer::Loopback4
            | Answer::Loopback6
            | Answer::NoData
            | Answer::NxDomain
            | Answer::Refused => {}
        }
    }
}
