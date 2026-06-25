//! The counter store: a fixed-window limiter behind a trait so a Redis-backed implementation
//! can replace the in-memory one later (design pillar 2) without touching the gRPC path.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Current UNIX time in whole seconds; 0 if the clock is before the epoch (never, in practice).
pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A fixed-window counter. `incr` adds `hits` to the count for `key` in the window that
/// `now_unix` falls in and returns the running total. Crossing a window boundary resets the
/// count. `now_unix` is a parameter (not read internally) so tests are deterministic.
pub trait CounterStore: Send + Sync {
    fn incr(&self, key: &str, window_seconds: u64, hits: u64, now_unix: u64) -> u64;
}

const SHARDS: usize = 64;

#[derive(Clone, Copy)]
struct Slot {
    window: u64,
    count: u64,
}

/// In-memory sharded fixed-window store. Sharding by key hash keeps lock contention low without
/// pulling in a concurrent-map dependency. Counts are process-local and reset on restart.
pub struct InMemoryFixedWindow {
    shards: Vec<Mutex<HashMap<String, Slot>>>,
}

impl InMemoryFixedWindow {
    pub fn new() -> Self {
        let mut shards = Vec::with_capacity(SHARDS);
        for _ in 0..SHARDS {
            shards.push(Mutex::new(HashMap::new()));
        }
        Self { shards }
    }

    fn shard(&self, key: &str) -> &Mutex<HashMap<String, Slot>> {
        // FNV-1a — cheap, stable, good enough for shard selection.
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in key.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        let idx = (hash as usize) % SHARDS;
        &self.shards[idx]
    }
}

impl Default for InMemoryFixedWindow {
    fn default() -> Self {
        Self::new()
    }
}

impl CounterStore for InMemoryFixedWindow {
    fn incr(&self, key: &str, window_seconds: u64, hits: u64, now_unix: u64) -> u64 {
        let window = if window_seconds == 0 {
            now_unix
        } else {
            now_unix / window_seconds
        };
        let shard = self.shard(key);
        // A poisoned lock means a prior holder panicked; the counter is best-effort and
        // self-healing on the next window, so recover the guard rather than propagate a panic.
        let mut guard = match shard.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let slot = guard
            .entry(key.to_string())
            .or_insert(Slot { window, count: 0 });
        if slot.window != window {
            slot.window = window;
            slot.count = 0;
        }
        slot.count = slot.count.saturating_add(hits);
        slot.count
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::sync::Arc;

    // Windows are epoch-aligned: the window id is `now / window_seconds`, so a 60s window
    // spans `[960, 1020)` (id 16), then `[1020, 1080)` (id 17) — the standard fixed-window
    // bucketing an RLS uses (resets at wall-clock boundaries, not first-seen-relative).
    #[test]
    fn counts_up_within_a_window() {
        let store = InMemoryFixedWindow::new();
        assert_eq!(store.incr("k", 60, 1, 960), 1);
        assert_eq!(store.incr("k", 60, 1, 1010), 2);
        assert_eq!(store.incr("k", 60, 1, 1019), 3); // all in epoch window 16 = [960, 1020)
    }

    #[test]
    fn resets_on_window_boundary() {
        let store = InMemoryFixedWindow::new();
        assert_eq!(store.incr("k", 60, 1, 960), 1);
        assert_eq!(store.incr("k", 60, 1, 1019), 2); // still window 16
                                                     // 1020 / 60 = 17 -> new window, count resets.
        assert_eq!(store.incr("k", 60, 1, 1020), 1);
    }

    #[test]
    fn keys_are_independent() {
        let store = InMemoryFixedWindow::new();
        assert_eq!(store.incr("a", 60, 1, 1000), 1);
        assert_eq!(store.incr("b", 60, 1, 1000), 1);
        assert_eq!(store.incr("a", 60, 1, 1000), 2);
    }

    #[test]
    fn hits_addend_is_respected() {
        let store = InMemoryFixedWindow::new();
        assert_eq!(store.incr("k", 1, 5, 10), 5);
        assert_eq!(store.incr("k", 1, 3, 10), 8);
    }

    #[test]
    fn concurrent_increments_do_not_lose_counts() {
        let store = Arc::new(InMemoryFixedWindow::new());
        let mut handles = Vec::new();
        for _ in 0..8 {
            let store = Arc::clone(&store);
            handles.push(std::thread::spawn(move || {
                for _ in 0..1000 {
                    store.incr("hot", 3600, 1, 42);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(store.incr("hot", 3600, 0, 42), 8000);
    }
}
