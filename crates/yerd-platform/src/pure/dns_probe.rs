//! Compose and validate the loopback DNS probe that confirms a Linux resolver
//! backend really answers for Yerd's zone.
//!
//! Hand-rolled rather than delegating to a resolver stack: the caller only
//! needs to know whether something answers authoritatively with loopback, and
//! the privileged helper should not grow a DNS dependency for one packet.

/// Label prefixed to the TLD to form the probe name. Yerd's responder answers
/// every name under its zone, so a synthetic label avoids depending on any
/// site existing.
pub const PROBE_LABEL: &str = "yerd-resolver-probe";

/// Transaction ID ("YD"), echoed by the responder and re-checked on the way
/// back so a stray datagram cannot satisfy the probe.
const TXN_ID: [u8; 2] = [0x59, 0x44];

/// Compose an A query for `<PROBE_LABEL>.<tld>`. Returns `None` when a label
/// is too long to encode.
#[must_use]
pub fn compose_query(tld: &str) -> Option<Vec<u8>> {
    let mut packet = Vec::new();
    packet.extend_from_slice(&TXN_ID);
    packet.extend_from_slice(&[
        0x01, 0x00, // flags: recursion desired
        0x00, 0x01, // qdcount
        0, 0, // ancount
        0, 0, // nscount
        0, 0, // arcount
    ]);
    for label in [PROBE_LABEL, tld] {
        let len = u8::try_from(label.len()).ok()?;
        packet.push(len);
        packet.extend_from_slice(label.as_bytes());
    }
    packet.extend_from_slice(&[
        0, // root label
        0, 1, // qtype A
        0, 1, // qclass IN
    ]);
    Some(packet)
}

/// Whether `packet` answers [`compose_query`] with an `A 127.0.0.1` record.
///
/// Requires our transaction ID, the response bit, and `NOERROR`, then scans
/// for a resource record whose fixed 14-byte tail past the name reads
/// type `A`, class `IN`, a 4-byte RDATA length, and the loopback address.
/// Scanning sidesteps name-compression parsing; the RCODE and address checks
/// make a false positive from unrelated bytes implausible.
#[must_use]
pub fn response_has_loopback_a(packet: &[u8]) -> bool {
    packet.len() >= 12
        && packet.starts_with(&TXN_ID)
        && packet.get(2).is_some_and(|flags| flags & 0x80 != 0)
        && packet.get(3).map(|flags| flags & 0x0f) == Some(0)
        && packet.windows(14).any(|window| {
            window.starts_with(&[0, 1, 0, 1])
                && window.get(8..10) == Some([0, 4].as_slice())
                && window.ends_with(&[127, 0, 0, 1])
        })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn answer() -> Vec<u8> {
        let mut response = vec![0x59, 0x44, 0x81, 0x80, 0, 1, 0, 1, 0, 0, 0, 0];
        response.extend_from_slice(&[0xc0, 0x0c, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4, 127, 0, 0, 1]);
        response
    }

    #[test]
    fn query_encodes_probe_name_and_a_question() {
        let packet = compose_query("test").unwrap();
        assert!(packet.starts_with(&[0x59, 0x44, 0x01, 0x00, 0x00, 0x01]));
        assert_eq!(packet[12], 19);
        assert_eq!(&packet[13..32], PROBE_LABEL.as_bytes());
        assert_eq!(packet[32], 4);
        assert_eq!(&packet[33..37], b"test");
        assert_eq!(&packet[37..], &[0, 0, 1, 0, 1]);
    }

    #[test]
    fn query_rejects_an_unencodable_label() {
        assert!(compose_query(&"a".repeat(256)).is_none());
    }

    #[test]
    fn response_accepts_a_successful_loopback_answer() {
        assert!(response_has_loopback_a(&answer()));
    }

    #[test]
    fn response_rejects_error_rcode_foreign_id_and_wrong_address() {
        let mut nxdomain = answer();
        nxdomain[3] = 0x83;
        assert!(!response_has_loopback_a(&nxdomain));

        let mut foreign = answer();
        foreign[0] = 0x00;
        assert!(!response_has_loopback_a(&foreign));

        let mut elsewhere = answer();
        let last = elsewhere.len() - 4;
        elsewhere[last..].copy_from_slice(&[10, 0, 0, 1]);
        assert!(!response_has_loopback_a(&elsewhere));

        let mut query_only = answer();
        query_only[2] = 0x01;
        assert!(!response_has_loopback_a(&query_only));

        assert!(!response_has_loopback_a(&[]));
    }
}
