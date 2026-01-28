//! MCP Operation Cancellation Support
//!
//! Tracks active operations that can be cancelled via `notifications/cancelled`.
//! Uses tokio_util::sync::CancellationToken for proper async cancellation.

use dashmap::DashMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::debug;

use crate::mcp::protocol::JsonRpcId;

/// Manages cancellation tokens for active operations
pub struct CancellationManager {
    /// Active cancellation tokens indexed by request ID
    tokens: DashMap<JsonRpcId, CancellationToken>,
}

impl CancellationManager {
    /// Create a new cancellation manager
    pub fn new() -> Self {
        Self { tokens: DashMap::new() }
    }

    /// Register a new cancellable operation
    ///
    /// Returns a CancellationToken that will be triggered if the operation is cancelled.
    /// The caller should check `token.is_cancelled()` periodically during long-running operations.
    pub fn register(&self, request_id: JsonRpcId) -> CancellationToken {
        let token = CancellationToken::new();
        self.tokens.insert(request_id.clone(), token.clone());

        debug!(request_id = ?request_id, "Registered cancellable operation");

        token
    }

    /// Cancel an operation by request ID
    ///
    /// Returns `true` if the operation was found and cancelled, `false` otherwise.
    pub fn cancel(&self, request_id: &JsonRpcId) -> bool {
        if let Some(entry) = self.tokens.get(request_id) {
            entry.cancel();
            debug!(request_id = ?request_id, "Cancelled operation");
            true
        } else {
            debug!(request_id = ?request_id, "Operation not found for cancellation");
            false
        }
    }

    /// Mark an operation as complete and remove its token
    ///
    /// Should be called after an operation completes (successfully or with error)
    /// to clean up the cancellation token.
    pub fn complete(&self, request_id: &JsonRpcId) {
        if self.tokens.remove(request_id).is_some() {
            debug!(request_id = ?request_id, "Completed and removed operation");
        }
    }

    /// Check if an operation has been cancelled
    pub fn is_cancelled(&self, request_id: &JsonRpcId) -> bool {
        self.tokens.get(request_id).map(|t| t.is_cancelled()).unwrap_or(false)
    }

    /// Get the number of active operations
    pub fn active_count(&self) -> usize {
        self.tokens.len()
    }

    /// Check if an operation is registered
    pub fn is_registered(&self, request_id: &JsonRpcId) -> bool {
        self.tokens.contains_key(request_id)
    }

    /// Get a child token for an operation
    ///
    /// Returns a new token that will be cancelled when either the parent token
    /// or this operation's token is cancelled. Useful for creating sub-operations.
    pub fn child_token(&self, request_id: &JsonRpcId) -> Option<CancellationToken> {
        self.tokens.get(request_id).map(|t| t.child_token())
    }
}

impl Default for CancellationManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Arc wrapper for shared cancellation manager
pub type SharedCancellationManager = Arc<CancellationManager>;

/// Create a new shared cancellation manager
pub fn create_cancellation_manager() -> SharedCancellationManager {
    Arc::new(CancellationManager::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_operation() {
        let manager = CancellationManager::new();
        let request_id = JsonRpcId::Number(1);

        let token = manager.register(request_id.clone());

        assert!(manager.is_registered(&request_id));
        assert!(!token.is_cancelled());
        assert_eq!(manager.active_count(), 1);
    }

    #[test]
    fn test_cancel_operation() {
        let manager = CancellationManager::new();
        let request_id = JsonRpcId::Number(1);

        let token = manager.register(request_id.clone());
        assert!(!token.is_cancelled());

        let cancelled = manager.cancel(&request_id);
        assert!(cancelled);
        assert!(token.is_cancelled());
        assert!(manager.is_cancelled(&request_id));
    }

    #[test]
    fn test_cancel_nonexistent_operation() {
        let manager = CancellationManager::new();
        let request_id = JsonRpcId::Number(999);

        let cancelled = manager.cancel(&request_id);
        assert!(!cancelled);
    }

    #[test]
    fn test_complete_operation() {
        let manager = CancellationManager::new();
        let request_id = JsonRpcId::Number(1);

        let _token = manager.register(request_id.clone());
        assert!(manager.is_registered(&request_id));

        manager.complete(&request_id);
        assert!(!manager.is_registered(&request_id));
        assert_eq!(manager.active_count(), 0);
    }

    #[test]
    fn test_is_cancelled_nonexistent() {
        let manager = CancellationManager::new();
        let request_id = JsonRpcId::Number(999);

        // Non-existent operations should return false
        assert!(!manager.is_cancelled(&request_id));
    }

    #[test]
    fn test_multiple_operations() {
        let manager = CancellationManager::new();
        let id1 = JsonRpcId::Number(1);
        let id2 = JsonRpcId::String("request-2".to_string());
        let id3 = JsonRpcId::Number(3);

        let token1 = manager.register(id1.clone());
        let token2 = manager.register(id2.clone());
        let token3 = manager.register(id3.clone());

        assert_eq!(manager.active_count(), 3);

        // Cancel only id2
        manager.cancel(&id2);
        assert!(!token1.is_cancelled());
        assert!(token2.is_cancelled());
        assert!(!token3.is_cancelled());

        // Complete id1
        manager.complete(&id1);
        assert_eq!(manager.active_count(), 2);
        assert!(!manager.is_registered(&id1));
    }

    #[test]
    fn test_child_token() {
        let manager = CancellationManager::new();
        let request_id = JsonRpcId::Number(1);

        let _parent = manager.register(request_id.clone());
        let child = manager.child_token(&request_id);

        assert!(child.is_some());
        let child_token = child.unwrap();
        assert!(!child_token.is_cancelled());

        // Cancel parent should cancel child
        manager.cancel(&request_id);
        assert!(child_token.is_cancelled());
    }

    #[test]
    fn test_child_token_nonexistent() {
        let manager = CancellationManager::new();
        let request_id = JsonRpcId::Number(999);

        let child = manager.child_token(&request_id);
        assert!(child.is_none());
    }

    #[test]
    fn test_json_rpc_id_string() {
        let manager = CancellationManager::new();
        let request_id = JsonRpcId::String("test-request".to_string());

        let token = manager.register(request_id.clone());
        assert!(manager.is_registered(&request_id));

        manager.cancel(&request_id);
        assert!(token.is_cancelled());
    }

    #[tokio::test]
    async fn test_cancellation_token_await() {
        let manager = CancellationManager::new();
        let request_id = JsonRpcId::Number(1);

        let token = manager.register(request_id.clone());

        // Spawn a task that waits for cancellation
        let token_clone = token.clone();
        let handle = tokio::spawn(async move {
            token_clone.cancelled().await;
            "cancelled"
        });

        // Small delay to ensure task is waiting
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Cancel the operation
        manager.cancel(&request_id);

        // Task should complete
        let result = handle.await.expect("Task should complete");
        assert_eq!(result, "cancelled");
    }

    #[test]
    fn test_create_cancellation_manager() {
        let manager = create_cancellation_manager();
        assert_eq!(manager.active_count(), 0);
    }

    #[test]
    fn test_concurrent_access() {
        use std::sync::Arc;
        use std::thread;

        let manager = Arc::new(CancellationManager::new());

        let handles: Vec<_> = (0..10)
            .map(|i| {
                let manager = Arc::clone(&manager);
                thread::spawn(move || {
                    for j in 0..100 {
                        let request_id = JsonRpcId::Number((i * 100 + j) as i64);
                        let _token = manager.register(request_id.clone());
                        manager.complete(&request_id);
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().expect("Thread panicked");
        }

        // All operations should be completed
        assert_eq!(manager.active_count(), 0);
    }
}
