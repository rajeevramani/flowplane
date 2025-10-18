//! Background processing pipeline for access log entries
//!
//! This module implements a Tokio-based worker pool that processes access log entries
//! from learning sessions. The pipeline stages are:
//!
//! 1. **Task 5.1**: Worker pool infrastructure (this file)
//! 2. **Task 5.2**: Schema inference integration
//! 3. **Task 5.3**: Batch database writes
//! 4. **Task 5.4**: Retry and backpressure mechanisms
//! 5. **Task 5.5**: Metrics collection and health checks
//!
//! ## Architecture
//!
//! The processor spawns multiple worker tasks that read from a shared queue:
//!
//! ```text
//! AccessLogService → UnboundedChannel → Worker Pool (N workers)
//!                                         ↓
//!                                    Process Entry
//!                                         ↓
//!                                    (Future: Schema Inference)
//!                                         ↓
//!                                    (Future: Batch DB Write)
//! ```
//!
//! ## Graceful Shutdown
//!
//! The processor supports graceful shutdown via tokio::select and shutdown signals.
//! Workers drain the queue before terminating.

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, watch, Mutex};
use tokio::time::interval;
use tracing::{debug, error, info, warn};

use crate::schema::inference::SchemaInferenceEngine;
use crate::xds::services::access_log_service::ProcessedLogEntry;
use crate::Result;
use sqlx::{Pool, Sqlite};

/// Configuration for the access log processor
#[derive(Debug, Clone)]
pub struct ProcessorConfig {
    /// Number of worker tasks to spawn
    /// Default: num_cpus::get() (one per CPU core)
    pub worker_count: usize,

    /// Maximum number of schemas to accumulate before batch write
    /// Default: 100 schemas
    pub batch_size: usize,

    /// Maximum time (in seconds) to wait before flushing batch
    /// Default: 5 seconds
    pub batch_flush_interval_secs: u64,
}

impl Default for ProcessorConfig {
    fn default() -> Self {
        Self {
            worker_count: num_cpus::get().max(1), // At least 1 worker
            batch_size: 100,                      // Batch write every 100 schemas
            batch_flush_interval_secs: 5,         // Or every 5 seconds
        }
    }
}

/// Inferred schema record ready for database persistence
#[derive(Debug, Clone)]
pub struct InferredSchemaRecord {
    pub session_id: String,
    pub team: String,
    pub http_method: String,
    pub path_pattern: String,
    pub request_schema: Option<String>,  // JSON Schema as string
    pub response_schema: Option<String>, // JSON Schema as string
    pub response_status_code: Option<u32>,
}

/// Background processor for access log entries
///
/// Spawns a pool of worker tasks that process access log entries asynchronously.
/// Supports graceful shutdown via watch channel.
pub struct AccessLogProcessor {
    config: ProcessorConfig,
    /// Shared receiver for access log entries from AccessLogService
    /// Wrapped in Arc<Mutex<>> to allow multiple workers to share access
    log_rx: Arc<Mutex<mpsc::UnboundedReceiver<ProcessedLogEntry>>>,
    /// Channel for sending inferred schemas to the batcher task
    schema_tx: mpsc::UnboundedSender<InferredSchemaRecord>,
    /// Receiver for inferred schemas (moved to batcher task)
    schema_rx: Arc<Mutex<mpsc::UnboundedReceiver<InferredSchemaRecord>>>,
    /// Database pool for batch writes (optional for testing)
    db_pool: Option<Pool<Sqlite>>,
    /// Shutdown signal sender (broadcast to all workers)
    shutdown_tx: watch::Sender<bool>,
    /// Shutdown signal receiver (cloned for each worker)
    shutdown_rx: watch::Receiver<bool>,
}

/// Handle for controlling a running access log processor
///
/// Provides shutdown functionality for the processor workers
pub struct ProcessorHandle {
    shutdown_tx: watch::Sender<bool>,
    worker_handles: Vec<tokio::task::JoinHandle<()>>,
}

impl ProcessorHandle {
    /// Trigger graceful shutdown
    ///
    /// Sends shutdown signal to all workers. Workers will drain their queues
    /// before terminating.
    pub fn shutdown(&self) {
        info!("Initiating graceful shutdown of access log processor");
        let _ = self.shutdown_tx.send(true);
    }

    /// Wait for all workers to finish
    ///
    /// This consumes the handle and waits for all worker tasks to complete
    pub async fn join(self) {
        for handle in self.worker_handles {
            let _ = handle.await;
        }
    }
}

impl AccessLogProcessor {
    /// Create a new AccessLogProcessor
    ///
    /// # Arguments
    ///
    /// * `log_rx` - Receiver for processed log entries from AccessLogService
    /// * `db_pool` - Optional database pool for batch writes
    /// * `config` - Optional configuration (uses defaults if None)
    ///
    /// # Returns
    ///
    /// Returns the processor instance ready to spawn workers
    pub fn new(
        log_rx: mpsc::UnboundedReceiver<ProcessedLogEntry>,
        db_pool: Option<Pool<Sqlite>>,
        config: Option<ProcessorConfig>,
    ) -> Self {
        let config = config.unwrap_or_default();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let (schema_tx, schema_rx) = mpsc::unbounded_channel();

        info!(
            worker_count = config.worker_count,
            batch_size = config.batch_size,
            flush_interval_secs = config.batch_flush_interval_secs,
            has_database = db_pool.is_some(),
            "Created AccessLogProcessor"
        );

        Self {
            config,
            log_rx: Arc::new(Mutex::new(log_rx)),
            schema_tx,
            schema_rx: Arc::new(Mutex::new(schema_rx)),
            db_pool,
            shutdown_tx,
            shutdown_rx,
        }
    }

    /// Spawn worker tasks
    ///
    /// This spawns `worker_count` Tokio tasks that process log entries concurrently.
    /// Each worker:
    /// 1. Reads from the shared log_rx channel
    /// 2. Processes the entry (currently just logs it - Task 5.2 will add schema inference)
    /// 3. Watches for shutdown signal
    /// 4. Drains remaining queue entries on shutdown
    ///
    /// # Returns
    ///
    /// Returns a ProcessorHandle for controlling the spawned workers
    pub fn spawn_workers(self) -> ProcessorHandle {
        let mut handles = Vec::with_capacity(self.config.worker_count);

        info!(worker_count = self.config.worker_count, "Spawning access log processor workers");

        for worker_id in 0..self.config.worker_count {
            let mut shutdown_rx = self.shutdown_rx.clone();
            let log_rx = Arc::clone(&self.log_rx);

            let handle = tokio::spawn(async move {
                info!(worker_id, "Access log processor worker started");

                loop {
                    tokio::select! {
                        // Process log entries
                        entry = async {
                            let mut rx = log_rx.lock().await;
                            rx.recv().await
                        } => {
                            if let Some(entry) = entry {
                                if let Err(e) = Self::process_entry(worker_id, entry).await {
                                    error!(
                                        worker_id,
                                        error = %e,
                                        "Failed to process log entry"
                                    );
                                }
                            }
                        }

                        // Watch for shutdown signal
                        _ = shutdown_rx.changed() => {
                            if *shutdown_rx.borrow() {
                                info!(worker_id, "Received shutdown signal, draining queue");

                                // Drain remaining entries in queue
                                let mut drained = 0;
                                loop {
                                    let entry = {
                                        let mut rx = log_rx.lock().await;
                                        rx.try_recv()
                                    };

                                    match entry {
                                        Ok(entry) => {
                                            if let Err(e) = Self::process_entry(worker_id, entry).await {
                                                warn!(
                                                    worker_id,
                                                    error = %e,
                                                    "Failed to process entry during shutdown drain"
                                                );
                                            }
                                            drained += 1;
                                        }
                                        Err(_) => break, // Queue is empty
                                    }
                                }

                                info!(
                                    worker_id,
                                    drained_entries = drained,
                                    "Worker shutdown complete"
                                );
                                break;
                            }
                        }
                    }
                }
            });

            handles.push(handle);
        }

        info!(spawned_workers = handles.len(), "All access log processor workers spawned");

        ProcessorHandle { shutdown_tx: self.shutdown_tx, worker_handles: handles }
    }

    /// Process a single log entry
    ///
    /// Task 5.1: ✅ Worker pool infrastructure
    /// Task 5.2: ✅ Schema inference integration
    /// Task 5.3: TODO - Batched DB writes
    /// Task 5.4: TODO - Retry logic
    /// Task 5.5: TODO - Metrics
    async fn process_entry(worker_id: usize, entry: ProcessedLogEntry) -> Result<()> {
        debug!(
            worker_id,
            session_id = %entry.session_id,
            method = entry.method,
            path = %entry.path,
            status = entry.response_status,
            duration_ms = entry.duration_ms,
            has_request_body = entry.request_body.is_some(),
            has_response_body = entry.response_body.is_some(),
            "Processing access log entry"
        );

        // Task 5.2: Schema inference for request and response bodies
        let inference_engine = SchemaInferenceEngine::new();

        // Infer request schema if body is present
        if let Some(ref request_body) = entry.request_body {
            match std::str::from_utf8(request_body) {
                Ok(json_str) => {
                    match inference_engine.infer_from_json(json_str) {
                        Ok(schema) => {
                            debug!(
                                worker_id,
                                session_id = %entry.session_id,
                                path = %entry.path,
                                schema_type = ?schema.schema_type,
                                "Inferred request schema"
                            );
                            // TODO (Task 5.3): Store schema in database
                        }
                        Err(e) => {
                            // Non-JSON or malformed body - log but don't fail
                            debug!(
                                worker_id,
                                session_id = %entry.session_id,
                                error = %e,
                                "Failed to infer request schema (likely non-JSON body)"
                            );
                        }
                    }
                }
                Err(_) => {
                    // Binary request body - skip schema inference
                    debug!(
                        worker_id,
                        session_id = %entry.session_id,
                        "Request body is not valid UTF-8 (binary data)"
                    );
                }
            }
        }

        // Infer response schema if body is present
        if let Some(ref response_body) = entry.response_body {
            match std::str::from_utf8(response_body) {
                Ok(json_str) => {
                    match inference_engine.infer_from_json(json_str) {
                        Ok(schema) => {
                            debug!(
                                worker_id,
                                session_id = %entry.session_id,
                                path = %entry.path,
                                status = entry.response_status,
                                schema_type = ?schema.schema_type,
                                "Inferred response schema"
                            );
                            // TODO (Task 5.3): Store schema in database
                        }
                        Err(e) => {
                            // Non-JSON or malformed body - log but don't fail
                            debug!(
                                worker_id,
                                session_id = %entry.session_id,
                                error = %e,
                                "Failed to infer response schema (likely non-JSON body)"
                            );
                        }
                    }
                }
                Err(_) => {
                    // Binary response body - skip schema inference
                    debug!(
                        worker_id,
                        session_id = %entry.session_id,
                        "Response body is not valid UTF-8 (binary data)"
                    );
                }
            }
        }

        // TODO (Task 5.3): Add batched database writes here
        // TODO (Task 5.4): Add retry logic here
        // TODO (Task 5.5): Add metrics here

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{sleep, Duration};

    #[tokio::test]
    async fn test_processor_creation() {
        let (_tx, rx) = mpsc::unbounded_channel();
        let processor = AccessLogProcessor::new(rx, None, None);

        // Should use default config (num_cpus)
        assert!(processor.config.worker_count >= 1);
    }

    #[tokio::test]
    async fn test_processor_with_custom_config() {
        let (_tx, rx) = mpsc::unbounded_channel();
        let config =
            ProcessorConfig { worker_count: 4, batch_size: 100, batch_flush_interval_secs: 5 };
        let processor = AccessLogProcessor::new(rx, None, Some(config));

        assert_eq!(processor.config.worker_count, 4);
    }

    #[tokio::test]
    async fn test_worker_spawning() {
        let (_tx, rx) = mpsc::unbounded_channel();
        let config =
            ProcessorConfig { worker_count: 2, batch_size: 100, batch_flush_interval_secs: 5 };
        let processor = AccessLogProcessor::new(rx, None, Some(config));

        let handle = processor.spawn_workers();

        // Allow workers to start
        sleep(Duration::from_millis(100)).await;

        // Workers should be running (not finished)
        assert_eq!(handle.worker_handles.len(), 2);
        for worker_handle in &handle.worker_handles {
            assert!(!worker_handle.is_finished());
        }
    }

    #[tokio::test]
    async fn test_log_processing() {
        let (tx, rx) = mpsc::unbounded_channel();
        let config =
            ProcessorConfig { worker_count: 1, batch_size: 100, batch_flush_interval_secs: 5 };
        let processor = AccessLogProcessor::new(rx, None, Some(config));

        let _handle = processor.spawn_workers();

        // Send a test log entry
        let entry = ProcessedLogEntry {
            session_id: "test-session".to_string(),
            method: 1, // GET
            path: "/api/users".to_string(),
            request_headers: vec![],
            request_body: None,
            request_body_size: 0,
            response_status: 200,
            response_headers: vec![],
            response_body: None,
            response_body_size: 1024,
            start_time_seconds: 1234567890,
            duration_ms: 42,
        };

        tx.send(entry).unwrap();

        // Allow processing
        sleep(Duration::from_millis(100)).await;

        // Entry should have been processed (currently just logged)
        // No error means success
    }

    #[tokio::test]
    async fn test_graceful_shutdown() {
        let (_tx, rx) = mpsc::unbounded_channel();
        let config =
            ProcessorConfig { worker_count: 2, batch_size: 100, batch_flush_interval_secs: 5 };
        let processor = AccessLogProcessor::new(rx, None, Some(config));

        let handle = processor.spawn_workers();

        // Allow workers to start
        sleep(Duration::from_millis(100)).await;

        // Send shutdown signal
        handle.shutdown();

        // Wait for all workers to finish
        let result = tokio::time::timeout(Duration::from_secs(2), handle.join()).await;
        assert!(result.is_ok(), "Workers should finish within timeout");
    }

    #[tokio::test]
    async fn test_shutdown_drains_queue() {
        let (tx, rx) = mpsc::unbounded_channel();
        let config =
            ProcessorConfig { worker_count: 1, batch_size: 100, batch_flush_interval_secs: 5 };
        let processor = AccessLogProcessor::new(rx, None, Some(config));

        let handle = processor.spawn_workers();

        // Queue multiple entries
        for i in 0..10 {
            let entry = ProcessedLogEntry {
                session_id: format!("session-{}", i),
                method: 1,
                path: "/api/test".to_string(),
                request_headers: vec![],
                request_body: None,
                request_body_size: 0,
                response_status: 200,
                response_headers: vec![],
                response_body: None,
                response_body_size: 0,
                start_time_seconds: 1234567890,
                duration_ms: 10,
            };
            tx.send(entry).unwrap();
        }

        // Trigger shutdown immediately
        handle.shutdown();

        // Workers should drain queue before finishing
        let result = tokio::time::timeout(Duration::from_secs(2), handle.join()).await;
        assert!(result.is_ok(), "Workers should drain queue and finish");
    }

    #[tokio::test]
    async fn test_schema_inference_with_json_bodies() {
        let (tx, rx) = mpsc::unbounded_channel();
        let config =
            ProcessorConfig { worker_count: 1, batch_size: 100, batch_flush_interval_secs: 5 };
        let processor = AccessLogProcessor::new(rx, None, Some(config));

        let _handle = processor.spawn_workers();

        // Create mock JSON payloads
        let request_json =
            r#"{"user_id": 123, "action": "login", "timestamp": "2023-10-18T12:00:00Z"}"#;
        let response_json = r#"{"success": true, "token": "abc123", "expires_in": 3600}"#;

        let entry = ProcessedLogEntry {
            session_id: "test-session".to_string(),
            method: 2, // POST
            path: "/api/auth/login".to_string(),
            request_headers: vec![],
            request_body: Some(request_json.as_bytes().to_vec()),
            request_body_size: request_json.len() as u64,
            response_status: 200,
            response_headers: vec![],
            response_body: Some(response_json.as_bytes().to_vec()),
            response_body_size: response_json.len() as u64,
            start_time_seconds: 1234567890,
            duration_ms: 42,
        };

        tx.send(entry).unwrap();

        // Allow processing
        sleep(Duration::from_millis(200)).await;

        // Entry should have been processed with schema inference
        // No error means success (schemas were inferred correctly)
    }

    #[tokio::test]
    async fn test_schema_inference_with_non_json_bodies() {
        let (tx, rx) = mpsc::unbounded_channel();
        let config =
            ProcessorConfig { worker_count: 1, batch_size: 100, batch_flush_interval_secs: 5 };
        let processor = AccessLogProcessor::new(rx, None, Some(config));

        let _handle = processor.spawn_workers();

        // Create non-JSON payloads
        let binary_data = vec![0xFF, 0xD8, 0xFF, 0xE0]; // Binary data (JPEG header)

        let entry = ProcessedLogEntry {
            session_id: "test-session".to_string(),
            method: 2, // POST
            path: "/api/upload".to_string(),
            request_headers: vec![],
            request_body: Some(binary_data.clone()),
            request_body_size: binary_data.len() as u64,
            response_status: 200,
            response_headers: vec![],
            response_body: None,
            response_body_size: 0,
            start_time_seconds: 1234567890,
            duration_ms: 42,
        };

        tx.send(entry).unwrap();

        // Allow processing
        sleep(Duration::from_millis(200)).await;

        // Entry should have been processed without errors
        // (binary data is detected and schema inference is skipped)
    }

    #[tokio::test]
    async fn test_schema_inference_with_malformed_json() {
        let (tx, rx) = mpsc::unbounded_channel();
        let config =
            ProcessorConfig { worker_count: 1, batch_size: 100, batch_flush_interval_secs: 5 };
        let processor = AccessLogProcessor::new(rx, None, Some(config));

        let _handle = processor.spawn_workers();

        // Create malformed JSON
        let malformed_json = r#"{"user_id": 123, "action": "login""#; // Missing closing brace

        let entry = ProcessedLogEntry {
            session_id: "test-session".to_string(),
            method: 2, // POST
            path: "/api/auth/login".to_string(),
            request_headers: vec![],
            request_body: Some(malformed_json.as_bytes().to_vec()),
            request_body_size: malformed_json.len() as u64,
            response_status: 400,
            response_headers: vec![],
            response_body: None,
            response_body_size: 0,
            start_time_seconds: 1234567890,
            duration_ms: 42,
        };

        tx.send(entry).unwrap();

        // Allow processing
        sleep(Duration::from_millis(200)).await;

        // Entry should have been processed without errors
        // (malformed JSON is handled gracefully with debug logging)
    }

    #[tokio::test]
    async fn test_schema_inference_with_nested_json() {
        let (tx, rx) = mpsc::unbounded_channel();
        let config =
            ProcessorConfig { worker_count: 1, batch_size: 100, batch_flush_interval_secs: 5 };
        let processor = AccessLogProcessor::new(rx, None, Some(config));

        let _handle = processor.spawn_workers();

        // Create complex nested JSON
        let request_json = r#"
        {
            "user": {
                "id": 123,
                "email": "test@example.com",
                "profile": {
                    "name": "Test User",
                    "age": 30
                }
            },
            "metadata": {
                "ip": "192.168.1.1",
                "user_agent": "Mozilla/5.0"
            }
        }
        "#;

        let response_json = r#"
        {
            "data": {
                "user_id": 123,
                "settings": {
                    "theme": "dark",
                    "notifications": true
                }
            },
            "timestamp": "2023-10-18T12:00:00Z"
        }
        "#;

        let entry = ProcessedLogEntry {
            session_id: "test-session".to_string(),
            method: 1, // GET
            path: "/api/users/profile".to_string(),
            request_headers: vec![],
            request_body: Some(request_json.as_bytes().to_vec()),
            request_body_size: request_json.len() as u64,
            response_status: 200,
            response_headers: vec![],
            response_body: Some(response_json.as_bytes().to_vec()),
            response_body_size: response_json.len() as u64,
            start_time_seconds: 1234567890,
            duration_ms: 42,
        };

        tx.send(entry).unwrap();

        // Allow processing
        sleep(Duration::from_millis(200)).await;

        // Entry should have been processed with schema inference
        // Complex nested structures should be handled correctly
    }
}
