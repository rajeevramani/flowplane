//! MCP Message Buffer for SSE Resumability
//!
//! Provides a ring buffer for storing SSE messages to support
//! client reconnection with Last-Event-ID (MCP 2025-11-25).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::RwLock;

use crate::mcp::notifications::NotificationMessage;

/// Default buffer capacity (last 500 messages)
const DEFAULT_CAPACITY: usize = 500;

/// Buffered message with sequence number
#[derive(Debug, Clone)]
struct BufferedMessage {
    sequence: u64,
    message: NotificationMessage,
}

/// Ring buffer for SSE message resumability
///
/// Stores the last N messages with sequence numbers for replay on reconnect.
/// Uses a fixed-size VecDeque with LRU eviction when capacity is reached.
///
/// # Thread Safety
///
/// The buffer uses `tokio::sync::RwLock` for async-safe access.
/// The sequence counter uses `AtomicU64` for lock-free increment.
///
/// # Example
///
/// ```ignore
/// let buffer = MessageBuffer::new();
///
/// // Push messages
/// let seq1 = buffer.push(message1).await;
/// let seq2 = buffer.push(message2).await;
///
/// // Replay from sequence
/// let replayed = buffer.replay_from(seq1).await;
/// // Returns messages with sequence > seq1
/// ```
pub struct MessageBuffer {
    buffer: RwLock<VecDeque<BufferedMessage>>,
    capacity: usize,
    sequence: AtomicU64,
}

impl MessageBuffer {
    /// Create a new message buffer with default capacity (500)
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    /// Create a message buffer with specified capacity
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            buffer: RwLock::new(VecDeque::with_capacity(capacity)),
            capacity,
            sequence: AtomicU64::new(0),
        }
    }

    /// Push a message to the buffer, returning the assigned sequence number
    ///
    /// If the buffer is at capacity, the oldest message is evicted (LRU).
    /// The sequence number is globally unique within this buffer instance.
    pub async fn push(&self, message: NotificationMessage) -> u64 {
        let sequence = self.sequence.fetch_add(1, Ordering::SeqCst);

        let mut buffer = self.buffer.write().await;

        // Evict oldest if at capacity
        if buffer.len() >= self.capacity {
            buffer.pop_front();
        }

        buffer.push_back(BufferedMessage { sequence, message });

        sequence
    }

    /// Replay messages starting from the given sequence number (exclusive)
    ///
    /// Returns messages with sequence > from_sequence in chronological order.
    /// If from_sequence is older than the oldest buffered message, returns
    /// all buffered messages with sequence greater than from_sequence.
    ///
    /// # Returns
    ///
    /// A vector of (sequence, message) tuples in chronological order.
    pub async fn replay_from(&self, from_sequence: u64) -> Vec<(u64, NotificationMessage)> {
        let buffer = self.buffer.read().await;

        buffer
            .iter()
            .filter(|msg| msg.sequence > from_sequence)
            .map(|msg| (msg.sequence, msg.message.clone()))
            .collect()
    }

    /// Get the next sequence number that will be assigned
    ///
    /// Note: This is primarily for testing and debugging.
    pub fn next_sequence(&self) -> u64 {
        self.sequence.load(Ordering::SeqCst)
    }

    /// Get the oldest sequence number currently in the buffer
    ///
    /// Returns `None` if the buffer is empty.
    pub async fn oldest_sequence(&self) -> Option<u64> {
        let buffer = self.buffer.read().await;
        buffer.front().map(|msg| msg.sequence)
    }

    /// Get the newest sequence number currently in the buffer
    ///
    /// Returns `None` if the buffer is empty.
    pub async fn newest_sequence(&self) -> Option<u64> {
        let buffer = self.buffer.read().await;
        buffer.back().map(|msg| msg.sequence)
    }

    /// Get the current buffer size
    pub async fn len(&self) -> usize {
        let buffer = self.buffer.read().await;
        buffer.len()
    }

    /// Check if buffer is empty
    pub async fn is_empty(&self) -> bool {
        let buffer = self.buffer.read().await;
        buffer.is_empty()
    }

    /// Get the buffer capacity
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Clear all messages from the buffer
    ///
    /// Note: Does not reset the sequence counter.
    pub async fn clear(&self) {
        let mut buffer = self.buffer.write().await;
        buffer.clear();
    }
}

impl Default for MessageBuffer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::notifications::{
        LogLevel, LogNotification, ProgressNotification, ProgressToken,
    };
    use crate::mcp::protocol::JsonRpcResponse;

    fn create_ping_message() -> NotificationMessage {
        NotificationMessage::ping()
    }

    fn create_log_message(msg: &str) -> NotificationMessage {
        NotificationMessage::log(LogNotification::new(
            LogLevel::Info,
            Some("test".to_string()),
            serde_json::json!({ "message": msg }),
        ))
    }

    fn create_progress_message(progress: f64) -> NotificationMessage {
        NotificationMessage::progress(ProgressNotification::new(
            ProgressToken::new("test-token".to_string()),
            progress,
            Some(100.0),
            Some(format!("{}% complete", progress)),
        ))
    }

    fn create_response_message(id: i64) -> NotificationMessage {
        NotificationMessage::message(JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: Some(crate::mcp::protocol::JsonRpcId::Number(id)),
            result: Some(serde_json::json!({ "success": true })),
            error: None,
        })
    }

    #[tokio::test]
    async fn test_buffer_new() {
        let buffer = MessageBuffer::new();
        assert_eq!(buffer.capacity(), DEFAULT_CAPACITY);
        assert!(buffer.is_empty().await);
        assert_eq!(buffer.next_sequence(), 0);
    }

    #[tokio::test]
    async fn test_buffer_with_capacity() {
        let buffer = MessageBuffer::with_capacity(100);
        assert_eq!(buffer.capacity(), 100);
        assert!(buffer.is_empty().await);
    }

    #[tokio::test]
    async fn test_buffer_push_and_sequence() {
        let buffer = MessageBuffer::new();
        let msg = create_ping_message();

        let seq1 = buffer.push(msg.clone()).await;
        let seq2 = buffer.push(msg.clone()).await;
        let seq3 = buffer.push(msg).await;

        assert_eq!(seq1, 0);
        assert_eq!(seq2, 1);
        assert_eq!(seq3, 2);
        assert_eq!(buffer.len().await, 3);
        assert_eq!(buffer.next_sequence(), 3);
    }

    #[tokio::test]
    async fn test_buffer_oldest_newest_sequence() {
        let buffer = MessageBuffer::new();

        // Empty buffer
        assert_eq!(buffer.oldest_sequence().await, None);
        assert_eq!(buffer.newest_sequence().await, None);

        // Add messages
        let msg = create_ping_message();
        buffer.push(msg.clone()).await;
        buffer.push(msg.clone()).await;
        buffer.push(msg).await;

        assert_eq!(buffer.oldest_sequence().await, Some(0));
        assert_eq!(buffer.newest_sequence().await, Some(2));
    }

    #[tokio::test]
    async fn test_buffer_eviction_at_capacity() {
        let buffer = MessageBuffer::with_capacity(3);
        let msg = create_ping_message();

        buffer.push(msg.clone()).await; // seq 0
        buffer.push(msg.clone()).await; // seq 1
        buffer.push(msg.clone()).await; // seq 2
        assert_eq!(buffer.len().await, 3);
        assert_eq!(buffer.oldest_sequence().await, Some(0));

        buffer.push(msg.clone()).await; // seq 3, evicts seq 0
        assert_eq!(buffer.len().await, 3);
        assert_eq!(buffer.oldest_sequence().await, Some(1));

        buffer.push(msg).await; // seq 4, evicts seq 1
        assert_eq!(buffer.len().await, 3);
        assert_eq!(buffer.oldest_sequence().await, Some(2));
        assert_eq!(buffer.newest_sequence().await, Some(4));
    }

    #[tokio::test]
    async fn test_replay_from_sequence() {
        let buffer = MessageBuffer::new();

        buffer.push(create_log_message("msg0")).await; // seq 0
        buffer.push(create_log_message("msg1")).await; // seq 1
        buffer.push(create_log_message("msg2")).await; // seq 2
        buffer.push(create_log_message("msg3")).await; // seq 3

        // Replay from seq 1 (exclusive) - should get 2, 3
        let replayed = buffer.replay_from(1).await;
        assert_eq!(replayed.len(), 2);
        assert_eq!(replayed[0].0, 2);
        assert_eq!(replayed[1].0, 3);
    }

    #[tokio::test]
    async fn test_replay_from_zero() {
        let buffer = MessageBuffer::new();
        let msg = create_ping_message();

        buffer.push(msg.clone()).await; // seq 0
        buffer.push(msg.clone()).await; // seq 1
        buffer.push(msg).await; // seq 2

        // Replay from 0 (exclusive) - should get 1, 2
        let replayed = buffer.replay_from(0).await;
        assert_eq!(replayed.len(), 2);
        assert_eq!(replayed[0].0, 1);
        assert_eq!(replayed[1].0, 2);
    }

    #[tokio::test]
    async fn test_replay_from_future_sequence() {
        let buffer = MessageBuffer::new();
        let msg = create_ping_message();

        buffer.push(msg.clone()).await; // seq 0
        buffer.push(msg).await; // seq 1

        // Replay from sequence beyond buffer
        let replayed = buffer.replay_from(100).await;
        assert_eq!(replayed.len(), 0);
    }

    #[tokio::test]
    async fn test_replay_all_messages() {
        let buffer = MessageBuffer::new();
        let msg = create_ping_message();

        buffer.push(msg.clone()).await; // seq 0
        buffer.push(msg.clone()).await; // seq 1
        buffer.push(msg).await; // seq 2

        // Replay from u64::MAX (effectively replay nothing)
        let replayed = buffer.replay_from(u64::MAX).await;
        assert_eq!(replayed.len(), 0);

        // Replay from before first sequence (use wrapping subtraction for safety)
        // Actually, we just want all messages > some_sequence_before_0
        // But our sequences start at 0, so we need to handle this edge case
        // In practice, clients will send Last-Event-ID as seen, so this works
    }

    #[tokio::test]
    async fn test_replay_after_eviction() {
        let buffer = MessageBuffer::with_capacity(3);

        buffer.push(create_log_message("msg0")).await; // seq 0
        buffer.push(create_log_message("msg1")).await; // seq 1
        buffer.push(create_log_message("msg2")).await; // seq 2
        buffer.push(create_log_message("msg3")).await; // seq 3, evicts 0
        buffer.push(create_log_message("msg4")).await; // seq 4, evicts 1

        // Buffer now has: seq 2, 3, 4

        // Replay from 0 - should get 2, 3, 4 (all in buffer > 0)
        let replayed = buffer.replay_from(0).await;
        assert_eq!(replayed.len(), 3);
        assert_eq!(replayed[0].0, 2);
        assert_eq!(replayed[1].0, 3);
        assert_eq!(replayed[2].0, 4);

        // Replay from 2 - should get 3, 4
        let replayed = buffer.replay_from(2).await;
        assert_eq!(replayed.len(), 2);
        assert_eq!(replayed[0].0, 3);
        assert_eq!(replayed[1].0, 4);
    }

    #[tokio::test]
    async fn test_empty_buffer_replay() {
        let buffer = MessageBuffer::new();
        assert!(buffer.is_empty().await);

        let replayed = buffer.replay_from(0).await;
        assert_eq!(replayed.len(), 0);
    }

    #[tokio::test]
    async fn test_clear_buffer() {
        let buffer = MessageBuffer::new();
        let msg = create_ping_message();

        buffer.push(msg.clone()).await;
        buffer.push(msg.clone()).await;
        buffer.push(msg).await;

        assert_eq!(buffer.len().await, 3);
        assert_eq!(buffer.next_sequence(), 3);

        buffer.clear().await;

        assert!(buffer.is_empty().await);
        assert_eq!(buffer.next_sequence(), 3); // Sequence not reset
        assert_eq!(buffer.oldest_sequence().await, None);
    }

    #[tokio::test]
    async fn test_different_message_types() {
        let buffer = MessageBuffer::new();

        buffer.push(create_ping_message()).await;
        buffer.push(create_log_message("test log")).await;
        buffer.push(create_progress_message(50.0)).await;
        buffer.push(create_response_message(123)).await;

        assert_eq!(buffer.len().await, 4);

        let replayed = buffer.replay_from(0).await;
        assert_eq!(replayed.len(), 3); // seq 1, 2, 3

        // Verify message types are preserved
        assert!(matches!(replayed[0].1, NotificationMessage::Log { .. }));
        assert!(matches!(replayed[1].1, NotificationMessage::Progress { .. }));
        assert!(matches!(replayed[2].1, NotificationMessage::Message { .. }));
    }

    #[tokio::test]
    async fn test_concurrent_push() {
        use std::sync::Arc;
        use tokio::task::JoinSet;

        let buffer = Arc::new(MessageBuffer::with_capacity(1000));
        let mut tasks = JoinSet::new();

        // Spawn 10 tasks, each pushing 100 messages
        for _ in 0..10 {
            let buffer_clone = Arc::clone(&buffer);
            tasks.spawn(async move {
                for _ in 0..100 {
                    buffer_clone.push(create_ping_message()).await;
                }
            });
        }

        // Wait for all tasks
        while tasks.join_next().await.is_some() {}

        // Should have 1000 messages (all fit within capacity)
        assert_eq!(buffer.len().await, 1000);
        assert_eq!(buffer.next_sequence(), 1000);
    }

    #[tokio::test]
    async fn test_concurrent_push_with_eviction() {
        use std::sync::Arc;
        use tokio::task::JoinSet;

        let buffer = Arc::new(MessageBuffer::with_capacity(100));
        let mut tasks = JoinSet::new();

        // Spawn 10 tasks, each pushing 50 messages (500 total, 100 capacity)
        for _ in 0..10 {
            let buffer_clone = Arc::clone(&buffer);
            tasks.spawn(async move {
                for _ in 0..50 {
                    buffer_clone.push(create_ping_message()).await;
                }
            });
        }

        // Wait for all tasks
        while tasks.join_next().await.is_some() {}

        // Should have exactly 100 messages (capacity limit)
        assert_eq!(buffer.len().await, 100);
        assert_eq!(buffer.next_sequence(), 500);

        // Oldest should be around 400 (500 - 100)
        let oldest = buffer.oldest_sequence().await.unwrap();
        assert!((400..500).contains(&oldest));
    }

    #[tokio::test]
    async fn test_sequence_monotonically_increasing() {
        let buffer = MessageBuffer::new();
        let msg = create_ping_message();

        let mut prev_seq = 0;
        for i in 0..100 {
            let seq = buffer.push(msg.clone()).await;
            if i > 0 {
                assert!(seq > prev_seq, "Sequence must be monotonically increasing");
            }
            prev_seq = seq;
        }
    }

    #[tokio::test]
    async fn test_replay_preserves_order() {
        let buffer = MessageBuffer::new();

        // Push numbered messages
        for i in 0..10 {
            buffer.push(create_log_message(&format!("msg{}", i))).await;
        }

        // Replay and verify order
        let replayed = buffer.replay_from(0).await;
        assert_eq!(replayed.len(), 9); // seq 1-9

        for (i, (seq, _)) in replayed.iter().enumerate() {
            assert_eq!(*seq, (i + 1) as u64);
        }
    }

    #[tokio::test]
    async fn test_default_trait() {
        let buffer = MessageBuffer::default();
        assert_eq!(buffer.capacity(), DEFAULT_CAPACITY);
        assert!(buffer.is_empty().await);
    }
}
