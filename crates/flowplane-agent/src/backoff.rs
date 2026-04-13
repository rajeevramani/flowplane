//! Exponential backoff used when reconnecting to the CP diagnostics stream.
//!
//! Simple doubling with an absolute ceiling. No jitter — the CP is a single
//! sink per agent, so the thundering-herd concern that motivates jitter in
//! client-fanout scenarios does not apply here.

use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Backoff {
    min: Duration,
    max: Duration,
    current: Duration,
}

impl Backoff {
    pub fn new(min: Duration, max: Duration) -> Self {
        debug_assert!(min <= max, "backoff min must not exceed max");
        Self { min, max, current: min }
    }

    /// Return the current delay and advance the internal state (doubled,
    /// capped at `max`). Call this after a failed connection attempt.
    pub fn next_delay(&mut self) -> Duration {
        let delay = self.current;
        let doubled_ms = self.current.as_millis().saturating_mul(2);
        let cap_ms = self.max.as_millis();
        let capped_ms = doubled_ms.min(cap_ms);
        // Safe cast: cap_ms fits in u64 because it came from Duration.
        self.current = Duration::from_millis(capped_ms as u64);
        delay
    }

    /// Reset to the minimum delay. Call this after a successful connection.
    pub fn reset(&mut self) {
        self.current = self.min;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_at_min() {
        let mut b = Backoff::new(Duration::from_millis(100), Duration::from_secs(30));
        assert_eq!(b.next_delay(), Duration::from_millis(100));
    }

    #[test]
    fn doubles_on_each_call() {
        let mut b = Backoff::new(Duration::from_millis(100), Duration::from_secs(30));
        assert_eq!(b.next_delay(), Duration::from_millis(100));
        assert_eq!(b.next_delay(), Duration::from_millis(200));
        assert_eq!(b.next_delay(), Duration::from_millis(400));
        assert_eq!(b.next_delay(), Duration::from_millis(800));
    }

    #[test]
    fn caps_at_max() {
        let mut b = Backoff::new(Duration::from_millis(100), Duration::from_millis(500));
        assert_eq!(b.next_delay(), Duration::from_millis(100));
        assert_eq!(b.next_delay(), Duration::from_millis(200));
        assert_eq!(b.next_delay(), Duration::from_millis(400));
        // Next would be 800ms but max is 500ms.
        assert_eq!(b.next_delay(), Duration::from_millis(500));
        // And stays at 500ms forever.
        assert_eq!(b.next_delay(), Duration::from_millis(500));
        assert_eq!(b.next_delay(), Duration::from_millis(500));
    }

    #[test]
    fn reset_returns_to_min() {
        let mut b = Backoff::new(Duration::from_millis(100), Duration::from_secs(30));
        b.next_delay();
        b.next_delay();
        b.next_delay();
        b.reset();
        assert_eq!(b.next_delay(), Duration::from_millis(100));
    }

    #[test]
    fn very_large_current_does_not_panic() {
        // Exercise the saturating_mul path near (but under) u128 boundaries.
        // Purpose is to confirm we never panic on growth arithmetic.
        let mut b = Backoff::new(Duration::from_secs(1), Duration::from_secs(3_600));
        for _ in 0..64 {
            let _ = b.next_delay();
        }
    }
}
