//! Retention policy: bound the number of stored emails.

/// Default maximum number of captured emails to keep on disk.
pub const DEFAULT_CAP: usize = 200;

/// Given the current count (oldest-first ordering assumed) and a cap, return how
/// many of the oldest entries must be evicted to get back within the cap. Zero
/// when already within bounds.
#[must_use]
pub fn evict_count(current_len: usize, cap: usize) -> usize {
    current_len.saturating_sub(cap)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn under_cap_evicts_nothing() {
        assert_eq!(evict_count(5, 200), 0);
        assert_eq!(evict_count(200, 200), 0);
    }

    #[test]
    fn over_cap_evicts_the_overflow() {
        assert_eq!(evict_count(201, 200), 1);
        assert_eq!(evict_count(250, 200), 50);
    }
}
