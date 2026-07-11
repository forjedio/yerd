//! Pure port-allocation helper for picking a service instance's loopback port.
//!
//! No I/O: this only decides *which* ports to try, in order, skipping any already
//! spoken for. The daemon walks the returned candidates against the live
//! [`yerd_platform::PortBinder`] and takes the first that binds - so a stopped
//! instance whose configured port is in `reserved` still holds that port (the
//! next add advances past it), while a genuinely free port is taken on the first
//! bind. The `reserved` set is assembled by the daemon from every service's
//! configured port plus the mail/dumps/DNS/HTTP(S) ports.

use std::collections::BTreeSet;

/// Ascending candidate ports `>= start`, skipping any in `reserved` and skipping
/// port 0. Ends at [`u16::MAX`]; the caller binds each in turn until one succeeds.
pub fn candidate_ports(start: u16, reserved: &BTreeSet<u16>) -> impl Iterator<Item = u16> + '_ {
    (start.max(1)..=u16::MAX).filter(move |p| !reserved.contains(p))
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

    fn reserved(ports: &[u16]) -> BTreeSet<u16> {
        ports.iter().copied().collect()
    }

    #[test]
    fn first_candidate_is_start_when_free() {
        let r = reserved(&[]);
        assert_eq!(candidate_ports(8080, &r).next(), Some(8080));
    }

    #[test]
    fn skips_reserved_and_advances() {
        let r = reserved(&[8080, 8081]);
        let got: Vec<u16> = candidate_ports(8080, &r).take(2).collect();
        assert_eq!(got, vec![8082, 8083]);
    }

    /// A stopped instance still reserving 8081 means a new add on base 8080 lands
    /// on 8082, matching the "increment past a configured-but-off instance" rule.
    #[test]
    fn stopped_instance_port_still_reserved() {
        let r = reserved(&[8081]);
        let got: Vec<u16> = candidate_ports(8080, &r).take(2).collect();
        assert_eq!(got, vec![8080, 8082]);
    }

    #[test]
    fn never_yields_zero() {
        let r = reserved(&[]);
        assert_eq!(candidate_ports(0, &r).next(), Some(1));
    }

    #[test]
    fn empty_when_everything_from_start_reserved() {
        let mut r = BTreeSet::new();
        for p in 65534..=u16::MAX {
            r.insert(p);
        }
        assert_eq!(candidate_ports(65534, &r).next(), None);
    }
}
