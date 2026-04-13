//! Bounded FIFO queue with drop-oldest overflow.
//!
//! Used to buffer `DiagnosticsReport`s between the polling loop and the
//! gRPC stream loop. When the CP is unreachable the queue fills; once it
//! is full, every new push evicts the oldest buffered report and emits a
//! WARN log on the caller's side (we return a bool so the caller can log
//! with whatever context it has rather than importing tracing here).

use std::collections::VecDeque;
use tokio::sync::Mutex;

pub struct BoundedQueue<T> {
    inner: Mutex<VecDeque<T>>,
    cap: usize,
}

impl<T> BoundedQueue<T> {
    pub fn new(cap: usize) -> Self {
        let cap = cap.max(1);
        Self { inner: Mutex::new(VecDeque::with_capacity(cap)), cap }
    }

    /// Push an item. Returns `true` if the push caused an overflow eviction.
    pub async fn push(&self, item: T) -> bool {
        let mut q = self.inner.lock().await;
        let dropped = if q.len() >= self.cap {
            q.pop_front();
            true
        } else {
            false
        };
        q.push_back(item);
        dropped
    }

    /// Pop the oldest item, if any.
    pub async fn pop(&self) -> Option<T> {
        self.inner.lock().await.pop_front()
    }

    #[cfg(test)]
    async fn len(&self) -> usize {
        self.inner.lock().await.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn push_and_pop_in_fifo_order() {
        let q = BoundedQueue::new(4);
        assert!(!q.push(1u32).await);
        assert!(!q.push(2).await);
        assert!(!q.push(3).await);
        assert_eq!(q.pop().await, Some(1));
        assert_eq!(q.pop().await, Some(2));
        assert_eq!(q.pop().await, Some(3));
        assert_eq!(q.pop().await, None);
    }

    #[tokio::test]
    async fn drops_oldest_on_overflow() {
        let q = BoundedQueue::new(3);
        assert!(!q.push(1u32).await);
        assert!(!q.push(2).await);
        assert!(!q.push(3).await);
        // Next push fills past cap — should evict the oldest (1) and signal.
        assert!(q.push(4).await);
        // Order should now be 2, 3, 4.
        assert_eq!(q.pop().await, Some(2));
        assert_eq!(q.pop().await, Some(3));
        assert_eq!(q.pop().await, Some(4));
    }

    #[tokio::test]
    async fn cap_zero_is_coerced_to_one() {
        // We never want a zero-cap queue that silently swallows everything.
        let q = BoundedQueue::<u32>::new(0);
        assert!(!q.push(1).await);
        assert_eq!(q.len().await, 1);
    }
}
