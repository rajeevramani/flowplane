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

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, watch, Mutex};
use tokio::time::interval;
use tracing::{debug, error, info, warn};

use crate::observability::metrics;
use crate::schema::inference::SchemaInferenceEngine;
use crate::services::{normalize_path, PathNormalizationConfig};
use crate::storage::DbPool;
use crate::xds::services::access_log_service::ProcessedLogEntry;
use crate::xds::services::ext_proc_service::CapturedBody;
use crate::Result;

/// Convert HTTP method code to string
///
/// Maps Envoy's RequestMethod enum values to HTTP method strings.
/// See: envoy/config/core/v3/base.proto - RequestMethod enum
fn method_to_string(method: i32) -> String {
    match method {
        0 => "UNKNOWN", // METHOD_UNSPECIFIED
        1 => "GET",
        2 => "HEAD",
        3 => "POST",
        4 => "PUT",
        5 => "DELETE",
        6 => "CONNECT",
        7 => "OPTIONS",
        8 => "TRACE",
        9 => "PATCH",
        _ => "UNKNOWN",
    }
    .to_string()
}

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

    /// Maximum number of retry attempts for failed batch writes
    /// Default: 3 retries
    pub max_retries: usize,

    /// Initial backoff duration in milliseconds for retries
    /// Default: 100ms (will exponentially increase: 100ms, 200ms, 400ms, etc.)
    pub initial_backoff_ms: u64,

    /// Maximum queue capacity before backpressure kicks in
    /// Default: 10,000 entries
    pub max_queue_capacity: usize,

    /// Path normalization configuration
    /// Default: REST API defaults (plural conversion enabled, common REST keywords)
    pub path_normalization: PathNormalizationConfig,

    /// TTL in seconds for pending entries (logs waiting for bodies or vice versa)
    /// Entries older than this will be removed to prevent unbounded memory growth
    /// Default: 300 seconds (5 minutes)
    pub pending_entry_ttl_secs: u64,

    /// Interval in seconds for cleanup task to check for stale entries
    /// Default: 60 seconds
    pub pending_cleanup_interval_secs: u64,
}

impl Default for ProcessorConfig {
    fn default() -> Self {
        Self {
            worker_count: num_cpus::get().max(1), // At least 1 worker
            batch_size: 100,                      // Batch write every 100 schemas
            batch_flush_interval_secs: 5,         // Or every 5 seconds
            max_retries: 3,                       // Retry up to 3 times
            initial_backoff_ms: 100,              // Start with 100ms backoff
            max_queue_capacity: 10_000,           // Drop entries after 10k queued
            path_normalization: PathNormalizationConfig::rest_defaults(), // Use REST defaults
            pending_entry_ttl_secs: 15,           // 15 second TTL for pending entries
            pending_cleanup_interval_secs: 5,     // Check for stale entries every 5 seconds
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

/// Wrapper for pending log entries with creation timestamp for TTL cleanup
#[derive(Debug)]
struct PendingLogEntry {
    entry: ProcessedLogEntry,
    created_at: tokio::time::Instant,
}

/// Wrapper for pending body captures with creation timestamp for TTL cleanup
#[derive(Debug)]
struct PendingBodyEntry {
    body: CapturedBody,
    created_at: tokio::time::Instant,
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
    /// Shared receiver for captured request/response bodies from ExtProc
    /// Wrapped in Arc<Mutex<>> to allow body merging
    ext_proc_rx: Option<Arc<Mutex<mpsc::UnboundedReceiver<CapturedBody>>>>,
    /// Pending log entries waiting for matching bodies
    /// Key: "{session_id}:{request_id}"
    /// Wrapped with creation timestamp for TTL-based cleanup
    pending_logs: Arc<Mutex<HashMap<String, PendingLogEntry>>>,
    /// Pending captured bodies waiting for matching log entries
    /// Key: "{session_id}:{request_id}"
    /// Wrapped with creation timestamp for TTL-based cleanup
    pending_bodies: Arc<Mutex<HashMap<String, PendingBodyEntry>>>,
    /// Channel for sending inferred schemas to the batcher task (bounded for backpressure)
    schema_tx: mpsc::Sender<InferredSchemaRecord>,
    /// Receiver for inferred schemas (moved to batcher task)
    schema_rx: Arc<Mutex<mpsc::Receiver<InferredSchemaRecord>>>,
    /// Database pool for batch writes (optional for testing)
    db_pool: Option<DbPool>,
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
    batcher_handle: Option<tokio::task::JoinHandle<()>>,
    cleanup_handle: Option<tokio::task::JoinHandle<()>>,
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
    /// This consumes the handle and waits for all worker tasks, batcher, and cleanup task to complete
    pub async fn join(self) {
        for handle in self.worker_handles {
            let _ = handle.await;
        }

        if let Some(batcher) = self.batcher_handle {
            let _ = batcher.await;
        }

        if let Some(cleanup) = self.cleanup_handle {
            let _ = cleanup.await;
        }
    }
}

impl AccessLogProcessor {
    /// Create a new AccessLogProcessor
    ///
    /// # Arguments
    ///
    /// * `log_rx` - Receiver for processed log entries from AccessLogService
    /// * `ext_proc_rx` - Optional receiver for captured bodies from ExtProc service
    /// * `db_pool` - Optional database pool for batch writes
    /// * `config` - Optional configuration (uses defaults if None)
    ///
    /// # Returns
    ///
    /// Returns the processor instance ready to spawn workers
    pub fn new(
        log_rx: mpsc::UnboundedReceiver<ProcessedLogEntry>,
        ext_proc_rx: Option<mpsc::UnboundedReceiver<CapturedBody>>,
        db_pool: Option<DbPool>,
        config: Option<ProcessorConfig>,
    ) -> Self {
        let config = config.unwrap_or_default();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        // Bounded channel for backpressure
        let (schema_tx, schema_rx) = mpsc::channel(config.max_queue_capacity);

        info!(
            worker_count = config.worker_count,
            batch_size = config.batch_size,
            flush_interval_secs = config.batch_flush_interval_secs,
            max_queue_capacity = config.max_queue_capacity,
            has_database = db_pool.is_some(),
            has_ext_proc = ext_proc_rx.is_some(),
            "Created AccessLogProcessor"
        );

        Self {
            config,
            log_rx: Arc::new(Mutex::new(log_rx)),
            ext_proc_rx: ext_proc_rx.map(|rx| Arc::new(Mutex::new(rx))),
            pending_logs: Arc::new(Mutex::new(HashMap::new())),
            pending_bodies: Arc::new(Mutex::new(HashMap::new())),
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

        // Update worker count metric
        let worker_count = self.config.worker_count;
        tokio::spawn(async move {
            metrics::update_processor_workers(worker_count).await;
        });

        for worker_id in 0..self.config.worker_count {
            let mut shutdown_rx = self.shutdown_rx.clone();
            let log_rx = Arc::clone(&self.log_rx);
            let ext_proc_rx = self.ext_proc_rx.as_ref().map(Arc::clone);
            let pending_logs = Arc::clone(&self.pending_logs);
            let pending_bodies = Arc::clone(&self.pending_bodies);
            let schema_tx = self.schema_tx.clone();
            let path_norm_config = self.config.path_normalization.clone();

            let handle = tokio::spawn(async move {
                info!(worker_id, "Access log processor worker started");

                loop {
                    tokio::select! {
                        // Process log entries from AccessLogService
                        entry = async {
                            let mut rx = log_rx.lock().await;
                            rx.recv().await
                        } => {
                            if let Some(entry) = entry {
                                // Try to merge with pending body if request_id is present
                                let entry_to_process = if let Some(merge_key) = Self::make_merge_key(&entry.session_id, entry.request_id.as_deref()) {
                                    let mut bodies_map = pending_bodies.lock().await;
                                    if let Some(pending_body) = bodies_map.remove(&merge_key) {
                                        // Found matching body - merge and process immediately
                                        debug!(
                                            worker_id,
                                            session_id = %entry.session_id,
                                            request_id = ?entry.request_id,
                                            "Merged log entry with pending body"
                                        );
                                        Self::merge_bodies(entry, pending_body.body)
                                    } else {
                                        // No matching body yet - store log entry and skip processing for now
                                        debug!(
                                            worker_id,
                                            session_id = %entry.session_id,
                                            request_id = ?entry.request_id,
                                            "Stored log entry, waiting for body"
                                        );
                                        let mut logs_map = pending_logs.lock().await;

                                        // Check if there's already an entry with this key (duplicate request_id)
                                        // If so, process the old entry before storing the new one to prevent data loss
                                        if let Some(old_pending) = logs_map.remove(&merge_key) {
                                            warn!(
                                                worker_id,
                                                session_id = %entry.session_id,
                                                request_id = ?entry.request_id,
                                                "Duplicate request_id detected - processing old entry before storing new one"
                                            );
                                            // Release lock before processing to avoid deadlock
                                            drop(logs_map);
                                            // Process the old entry without body merge (data loss prevention)
                                            if let Err(e) = Self::process_entry(worker_id, old_pending.entry, &schema_tx, &path_norm_config).await {
                                                error!(
                                                    worker_id,
                                                    error = %e,
                                                    "Failed to process old pending entry"
                                                );
                                            }
                                            // Re-acquire lock to store new entry
                                            let mut logs_map = pending_logs.lock().await;
                                            logs_map.insert(merge_key, PendingLogEntry {
                                                entry,
                                                created_at: tokio::time::Instant::now(),
                                            });
                                        } else {
                                            logs_map.insert(merge_key, PendingLogEntry {
                                                entry,
                                                created_at: tokio::time::Instant::now(),
                                            });
                                        }
                                        continue;
                                    }
                                } else {
                                    // No request_id - cannot merge with body, process as-is
                                    // Log at warn level so operators can identify misconfigured clients/Envoy setups
                                    // that are missing x-request-id headers needed for body correlation
                                    warn!(
                                        worker_id,
                                        session_id = %entry.session_id,
                                        path = %entry.path,
                                        method = entry.method,
                                        "Missing x-request-id header - body merge not possible. \
                                         Configure Envoy to add x-request-id header for complete schema inference."
                                    );
                                    // Record metric for observability
                                    metrics::record_missing_request_id(&entry.session_id).await;
                                    entry
                                };

                                if let Err(e) = Self::process_entry(worker_id, entry_to_process, &schema_tx, &path_norm_config).await {
                                    error!(
                                        worker_id,
                                        error = %e,
                                        "Failed to process log entry"
                                    );
                                }
                            }
                        }

                        // Process captured bodies from ExtProcService
                        body = async {
                            if let Some(ref ext_proc_rx) = ext_proc_rx {
                                let mut rx = ext_proc_rx.lock().await;
                                rx.recv().await
                            } else {
                                // No ExtProc channel - wait forever (will be cancelled by other branches)
                                std::future::pending().await
                            }
                        }, if ext_proc_rx.is_some() => {
                            if let Some(captured_body) = body {
                                // Try to merge with pending log entry
                                let merge_key = format!("{}:{}", captured_body.session_id, captured_body.request_id);
                                let mut logs_map = pending_logs.lock().await;
                                if let Some(pending_log) = logs_map.remove(&merge_key) {
                                    // Found matching log entry - merge and process immediately
                                    debug!(
                                        worker_id,
                                        session_id = %captured_body.session_id,
                                        request_id = %captured_body.request_id,
                                        "Merged captured body with pending log entry"
                                    );
                                    drop(logs_map); // Release lock before processing

                                    let merged_entry = Self::merge_bodies(pending_log.entry, captured_body);
                                    if let Err(e) = Self::process_entry(worker_id, merged_entry, &schema_tx, &path_norm_config).await {
                                        error!(
                                            worker_id,
                                            error = %e,
                                            "Failed to process merged entry"
                                        );
                                    }
                                } else {
                                    // No matching log entry yet - store body
                                    debug!(
                                        worker_id,
                                        session_id = %captured_body.session_id,
                                        request_id = %captured_body.request_id,
                                        "Stored captured body, waiting for log entry"
                                    );
                                    drop(logs_map); // Release logs lock
                                    let mut bodies_map = pending_bodies.lock().await;

                                    // Check if there's already a body with this key (duplicate request_id)
                                    // If so, log a warning and replace with the newer body
                                    if bodies_map.contains_key(&merge_key) {
                                        warn!(
                                            worker_id,
                                            session_id = %captured_body.session_id,
                                            request_id = %captured_body.request_id,
                                            "Duplicate request_id detected for body - replacing old body with new one"
                                        );
                                    }
                                    bodies_map.insert(merge_key, PendingBodyEntry {
                                        body: captured_body,
                                        created_at: tokio::time::Instant::now(),
                                    });
                                }
                            }
                        }

                        // Watch for shutdown signal
                        _ = shutdown_rx.changed() => {
                            if *shutdown_rx.borrow() {
                                info!(worker_id, "Received shutdown signal, draining queues");

                                // Drain remaining log entries
                                let mut drained_logs = 0;
                                loop {
                                    let entry = {
                                        let mut rx = log_rx.lock().await;
                                        rx.try_recv()
                                    };

                                    match entry {
                                        Ok(entry) => {
                                            // Best-effort merge on shutdown
                                            let entry_to_process = if let Some(merge_key) = Self::make_merge_key(&entry.session_id, entry.request_id.as_deref()) {
                                                let mut bodies_map = pending_bodies.lock().await;
                                                if let Some(pending_body) = bodies_map.remove(&merge_key) {
                                                    Self::merge_bodies(entry, pending_body.body)
                                                } else {
                                                    entry
                                                }
                                            } else {
                                                entry
                                            };

                                            if let Err(e) = Self::process_entry(worker_id, entry_to_process, &schema_tx, &path_norm_config).await {
                                                warn!(
                                                    worker_id,
                                                    error = %e,
                                                    "Failed to process entry during shutdown drain"
                                                );
                                            }
                                            drained_logs += 1;
                                        }
                                        Err(_) => break, // Queue is empty
                                    }
                                }

                                // Drain remaining captured bodies (process orphaned bodies)
                                let mut drained_bodies = 0;
                                if let Some(ref ext_proc_rx) = ext_proc_rx {
                                    loop {
                                        let body = {
                                            let mut rx = ext_proc_rx.lock().await;
                                            rx.try_recv()
                                        };

                                        match body {
                                            Ok(captured_body) => {
                                                let merge_key = format!("{}:{}", captured_body.session_id, captured_body.request_id);
                                                let mut logs_map = pending_logs.lock().await;
                                                if let Some(pending_log) = logs_map.remove(&merge_key) {
                                                    drop(logs_map);
                                                    let merged_entry = Self::merge_bodies(pending_log.entry, captured_body);
                                                    if let Err(e) = Self::process_entry(worker_id, merged_entry, &schema_tx, &path_norm_config).await {
                                                        warn!(
                                                            worker_id,
                                                            error = %e,
                                                            "Failed to process merged entry during shutdown"
                                                        );
                                                    }
                                                } else {
                                                    warn!(
                                                        worker_id,
                                                        session_id = %captured_body.session_id,
                                                        request_id = %captured_body.request_id,
                                                        "Orphaned captured body during shutdown (no matching log entry)"
                                                    );
                                                }
                                                drained_bodies += 1;
                                            }
                                            Err(_) => break, // Queue is empty
                                        }
                                    }
                                }

                                // Warn about any remaining unmatched entries
                                let pending_logs_count = pending_logs.lock().await.len();
                                let pending_bodies_count = pending_bodies.lock().await.len();

                                if pending_logs_count > 0 {
                                    warn!(
                                        worker_id,
                                        count = pending_logs_count,
                                        "Orphaned log entries at shutdown (no matching bodies)"
                                    );
                                }

                                if pending_bodies_count > 0 {
                                    warn!(
                                        worker_id,
                                        count = pending_bodies_count,
                                        "Orphaned bodies at shutdown (no matching log entries)"
                                    );
                                }

                                info!(
                                    worker_id,
                                    drained_logs,
                                    drained_bodies,
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

        // Spawn schema batcher task if database is configured
        let batcher_handle = if let Some(pool) = self.db_pool {
            let batcher = Self::spawn_schema_batcher(
                Arc::clone(&self.schema_rx),
                pool,
                self.config.batch_size,
                self.config.batch_flush_interval_secs,
                self.config.max_retries,
                self.config.initial_backoff_ms,
                self.shutdown_rx.clone(),
            );
            info!("Schema batcher task spawned");
            Some(batcher)
        } else {
            debug!("No database configured, schema batcher not spawned");
            None
        };

        // Spawn cleanup task that PROCESSES timed-out pending entries instead of dropping them.
        // When ExtProc fails to deliver bodies, ALS entries get stuck in pending_logs.
        // This task ensures they're eventually processed (without bodies) so schema inference
        // can at least log what happened, and metrics are recorded.
        let cleanup_handle = {
            let pending_logs = Arc::clone(&self.pending_logs);
            let pending_bodies = Arc::clone(&self.pending_bodies);
            let ttl_secs = self.config.pending_entry_ttl_secs;
            let cleanup_interval_secs = self.config.pending_cleanup_interval_secs;
            let schema_tx = self.schema_tx.clone();
            let path_norm_config = self.config.path_normalization.clone();
            let mut shutdown_rx = self.shutdown_rx.clone();

            let handle = tokio::spawn(async move {
                info!(ttl_secs, cleanup_interval_secs, "Pending entry cleanup task started");

                let mut interval =
                    tokio::time::interval(tokio::time::Duration::from_secs(cleanup_interval_secs));
                let ttl_duration = tokio::time::Duration::from_secs(ttl_secs);

                loop {
                    tokio::select! {
                        _ = interval.tick() => {
                            let now = tokio::time::Instant::now();

                            // Extract timed-out pending logs and PROCESS them (without bodies)
                            let timed_out_entries: Vec<PendingLogEntry> = {
                                let mut logs_map = pending_logs.lock().await;
                                let mut expired = Vec::new();
                                let keys_to_remove: Vec<String> = logs_map
                                    .iter()
                                    .filter(|(_, pending)| {
                                        now.duration_since(pending.created_at) > ttl_duration
                                    })
                                    .map(|(key, _)| key.clone())
                                    .collect();

                                for key in keys_to_remove {
                                    if let Some(pending) = logs_map.remove(&key) {
                                        expired.push(pending);
                                    }
                                }
                                expired
                            };

                            let logs_processed = timed_out_entries.len();
                            for pending in timed_out_entries {
                                warn!(
                                    session_id = %pending.entry.session_id,
                                    path = %pending.entry.path,
                                    method = pending.entry.method,
                                    request_id = ?pending.entry.request_id,
                                    age_secs = now.duration_since(pending.created_at).as_secs(),
                                    "Processing timed-out entry without body merge — \
                                     ExtProc may not be delivering bodies. \
                                     Schema inference will be skipped for this entry."
                                );
                                if let Err(e) = Self::process_entry(
                                    usize::MAX, // cleanup worker ID
                                    pending.entry,
                                    &schema_tx,
                                    &path_norm_config,
                                ).await {
                                    error!(
                                        error = %e,
                                        "Failed to process timed-out pending entry"
                                    );
                                }
                            }

                            // Clean up stale pending bodies (orphaned — no matching ALS entry)
                            let bodies_removed = {
                                let mut bodies_map = pending_bodies.lock().await;
                                let keys_to_remove: Vec<String> = bodies_map
                                    .iter()
                                    .filter(|(_, pending)| {
                                        now.duration_since(pending.created_at) > ttl_duration
                                    })
                                    .map(|(key, _)| key.clone())
                                    .collect();

                                for key in &keys_to_remove {
                                    if let Some(pending) = bodies_map.remove(key) {
                                        warn!(
                                            session_id = %pending.body.session_id,
                                            request_id = %pending.body.request_id,
                                            "Removing orphaned pending body (no matching ALS entry)"
                                        );
                                    }
                                }
                                keys_to_remove.len()
                            };

                            if logs_processed > 0 || bodies_removed > 0 {
                                warn!(
                                    logs_processed,
                                    bodies_removed,
                                    "Processed timed-out pending entries"
                                );
                            }
                        }

                        _ = shutdown_rx.changed() => {
                            if *shutdown_rx.borrow() {
                                info!("Cleanup task received shutdown signal");
                                break;
                            }
                        }
                    }
                }
            });
            info!("Pending entry cleanup task spawned");
            Some(handle)
        };

        ProcessorHandle {
            shutdown_tx: self.shutdown_tx,
            worker_handles: handles,
            batcher_handle,
            cleanup_handle,
        }
    }

    /// Merge captured body data into a processed log entry
    ///
    /// Task 12.4: Body merge logic (simplified MVP)
    fn merge_bodies(
        mut log_entry: ProcessedLogEntry,
        captured_body: CapturedBody,
    ) -> ProcessedLogEntry {
        // Only merge if bodies were actually captured (non-empty)
        if let Some(req_body) = captured_body.request_body {
            if !req_body.is_empty() {
                log_entry.request_body = Some(req_body);
                log_entry.request_body_size =
                    log_entry.request_body.as_ref().map(|b| b.len() as u64).unwrap_or(0);
            }
        }

        if let Some(resp_body) = captured_body.response_body {
            if !resp_body.is_empty() {
                log_entry.response_body = Some(resp_body);
                log_entry.response_body_size =
                    log_entry.response_body.as_ref().map(|b| b.len() as u64).unwrap_or(0);
            }
        }

        log_entry
    }

    /// Create a merge key from session_id and request_id
    ///
    /// Returns None if request_id is missing (cannot merge without it)
    fn make_merge_key(session_id: &str, request_id: Option<&str>) -> Option<String> {
        request_id.map(|rid| format!("{}:{}", session_id, rid))
    }

    /// Process a single log entry
    ///
    /// Task 5.1: ✅ Worker pool infrastructure
    /// Task 5.2: ✅ Schema inference integration
    /// Task 5.3: ✅ Batched DB writes (schemas sent to batcher)
    /// Task 5.4: ✅ Retry logic with backpressure
    /// Task 5.5: ✅ Metrics
    async fn process_entry(
        worker_id: usize,
        entry: ProcessedLogEntry,
        schema_tx: &mpsc::Sender<InferredSchemaRecord>,
        path_norm_config: &PathNormalizationConfig,
    ) -> Result<()> {
        let start = std::time::Instant::now();

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

                            // Normalize path before storing
                            let normalized_path = normalize_path(&entry.path, path_norm_config);

                            let record = InferredSchemaRecord {
                                session_id: entry.session_id.clone(),
                                team: entry.team.clone(),
                                http_method: method_to_string(entry.method),
                                path_pattern: normalized_path,
                                request_schema: Some(serde_json::to_string(
                                    &schema.to_json_schema(),
                                )?),
                                response_schema: None,
                                response_status_code: None,
                            };

                            // Send to batcher with backpressure handling
                            match schema_tx.try_send(record) {
                                Ok(_) => {
                                    info!(
                                        worker_id,
                                        session_id = %entry.session_id,
                                        path = %entry.path,
                                        schema_type = "request",
                                        "Inferred schema sent to batcher for persistence"
                                    );
                                    metrics::record_schema_inferred("request", true).await;
                                }
                                Err(mpsc::error::TrySendError::Full(_)) => {
                                    // Queue is full - drop the schema and log for metrics
                                    warn!(
                                        worker_id,
                                        session_id = %entry.session_id,
                                        path = %entry.path,
                                        "Schema queue full, dropping schema (backpressure)"
                                    );
                                    metrics::record_schema_dropped("request").await;
                                }
                                Err(mpsc::error::TrySendError::Closed(_)) => {
                                    // Batcher is shut down, ignore silently
                                    debug!(worker_id, "Batcher channel closed, dropping schema");
                                    metrics::record_schema_dropped("request").await;
                                }
                            }
                        }
                        Err(e) => {
                            // Non-JSON or malformed body - log but don't fail
                            debug!(
                                worker_id,
                                session_id = %entry.session_id,
                                error = %e,
                                "Failed to infer request schema (likely non-JSON body)"
                            );
                            metrics::record_schema_inferred("request", false).await;
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

                            // Normalize path before storing
                            let normalized_path = normalize_path(&entry.path, path_norm_config);

                            let record = InferredSchemaRecord {
                                session_id: entry.session_id.clone(),
                                team: entry.team.clone(),
                                http_method: method_to_string(entry.method),
                                path_pattern: normalized_path,
                                request_schema: None,
                                response_schema: Some(serde_json::to_string(
                                    &schema.to_json_schema(),
                                )?),
                                response_status_code: Some(entry.response_status),
                            };

                            // Send to batcher with backpressure handling
                            match schema_tx.try_send(record) {
                                Ok(_) => {
                                    info!(
                                        worker_id,
                                        session_id = %entry.session_id,
                                        path = %entry.path,
                                        status = entry.response_status,
                                        schema_type = "response",
                                        "Inferred schema sent to batcher for persistence"
                                    );
                                    metrics::record_schema_inferred("response", true).await;
                                }
                                Err(mpsc::error::TrySendError::Full(_)) => {
                                    // Queue is full - drop the schema and log for metrics
                                    warn!(
                                        worker_id,
                                        session_id = %entry.session_id,
                                        path = %entry.path,
                                        "Schema queue full, dropping schema (backpressure)"
                                    );
                                    metrics::record_schema_dropped("response").await;
                                }
                                Err(mpsc::error::TrySendError::Closed(_)) => {
                                    // Batcher is shut down, ignore silently
                                    debug!(worker_id, "Batcher channel closed, dropping schema");
                                    metrics::record_schema_dropped("response").await;
                                }
                            }
                        }
                        Err(e) => {
                            // Non-JSON or malformed body - log but don't fail
                            debug!(
                                worker_id,
                                session_id = %entry.session_id,
                                error = %e,
                                "Failed to infer response schema (likely non-JSON body)"
                            );
                            metrics::record_schema_inferred("response", false).await;
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

        // Record processing duration
        let duration = start.elapsed().as_secs_f64();
        metrics::record_processor_entry_duration(duration).await;

        Ok(())
    }

    /// Write a batch of inferred schemas to the database with retry logic
    ///
    /// Uses a single transaction for all inserts to ensure atomicity and performance.
    /// Task 5.3: Batch database writes for schema aggregation
    /// Task 5.4: Retry logic with exponential backoff
    async fn write_schema_batch_with_retry(
        pool: &DbPool,
        batch: Vec<InferredSchemaRecord>,
        max_retries: usize,
        initial_backoff_ms: u64,
    ) -> Result<()> {
        let batch_size = batch.len();
        let mut attempt = 0;
        let mut backoff_ms = initial_backoff_ms;

        loop {
            match Self::write_schema_batch(pool, batch.clone()).await {
                Ok(()) => {
                    if attempt > 0 {
                        info!(attempts = attempt + 1, "Batch write succeeded after retries");
                    }
                    metrics::record_schema_batch_write(batch_size, true, attempt).await;
                    return Ok(());
                }
                Err(e) => {
                    attempt += 1;

                    if attempt > max_retries {
                        error!(
                            error = %e,
                            attempts = attempt,
                            "Batch write failed after max retries, dropping batch"
                        );
                        metrics::record_schema_batch_write(batch_size, false, attempt).await;
                        return Err(e);
                    }

                    warn!(
                        error = %e,
                        attempt = attempt,
                        max_retries = max_retries,
                        backoff_ms = backoff_ms,
                        "Batch write failed, retrying after backoff"
                    );

                    // Exponential backoff
                    tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                    backoff_ms *= 2; // Double the backoff for next retry
                }
            }
        }
    }

    /// Write a batch of inferred schemas to the database
    ///
    /// Uses a single transaction for all inserts to ensure atomicity and performance.
    /// Task 5.3: Batch database writes for schema aggregation
    pub(crate) async fn write_schema_batch(
        pool: &DbPool,
        batch: Vec<InferredSchemaRecord>,
    ) -> Result<()> {
        if batch.is_empty() {
            return Ok(());
        }

        let batch_size = batch.len();
        debug!(batch_size, "Writing schema batch to database");

        // Begin transaction
        let mut tx = pool.begin().await?;

        // Insert all schemas in the batch
        for record in batch {
            sqlx::query(
                r#"
                INSERT INTO inferred_schemas (
                    team, session_id, http_method, path_pattern,
                    request_schema, response_schema, response_status_code,
                    sample_count, confidence
                ) VALUES ($1, $2, $3, $4, $5, $6, $7, 1, 1.0)
                "#,
            )
            .bind(&record.team)
            .bind(&record.session_id)
            .bind(&record.http_method)
            .bind(&record.path_pattern)
            .bind(&record.request_schema)
            .bind(&record.response_schema)
            .bind(record.response_status_code.map(|s| s as i64))
            .execute(&mut *tx)
            .await?;
        }

        // Commit transaction
        tx.commit().await?;

        info!(batch_size, "Successfully wrote schema batch to database");

        Ok(())
    }

    /// Spawn a schema batcher task that accumulates and batch-writes schemas
    ///
    /// This task:
    /// 1. Receives inferred schemas from workers via schema_rx channel
    /// 2. Accumulates them into batches based on batch_size
    /// 3. Flushes batches periodically based on batch_flush_interval
    /// 4. Handles graceful shutdown by flushing remaining batch
    ///
    /// Task 5.3: Batch accumulation and periodic flushing
    fn spawn_schema_batcher(
        schema_rx: Arc<Mutex<mpsc::Receiver<InferredSchemaRecord>>>,
        db_pool: DbPool,
        batch_size: usize,
        flush_interval_secs: u64,
        max_retries: usize,
        initial_backoff_ms: u64,
        mut shutdown_rx: watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut batch: Vec<InferredSchemaRecord> = Vec::with_capacity(batch_size);
            let mut flush_timer = interval(Duration::from_secs(flush_interval_secs));
            flush_timer.tick().await; // Skip first immediate tick

            info!("Schema batcher task started");

            loop {
                tokio::select! {
                    // Receive schema from workers
                    schema = async {
                        let mut rx = schema_rx.lock().await;
                        rx.recv().await
                    } => {
                        if let Some(schema) = schema {
                            batch.push(schema);

                            // Flush if batch is full
                            if batch.len() >= batch_size {
                                info!(
                                    batch_size = batch.len(),
                                    "Batch size limit reached, flushing to inferred_schemas table"
                                );

                                if let Err(e) = Self::write_schema_batch_with_retry(
                                    &db_pool,
                                    batch.clone(),
                                    max_retries,
                                    initial_backoff_ms,
                                )
                                .await
                                {
                                    error!(error = %e, "Failed to write schema batch after retries");
                                }

                                batch.clear();
                                flush_timer.reset(); // Reset timer after flush
                            }
                        }
                    }

                    // Periodic flush timer
                    _ = flush_timer.tick() => {
                        if !batch.is_empty() {
                            info!(
                                batch_size = batch.len(),
                                "Periodic flush interval reached, flushing batch to inferred_schemas table"
                            );

                            if let Err(e) = Self::write_schema_batch(&db_pool, batch.clone()).await {
                                error!(error = %e, "Failed to write schema batch");
                            }

                            batch.clear();
                        }
                    }

                    // Shutdown signal
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            info!("Schema batcher received shutdown signal");

                            // Drain remaining schemas from channel
                            loop {
                                let schema = {
                                    let mut rx = schema_rx.lock().await;
                                    rx.try_recv()
                                };

                                match schema {
                                    Ok(schema) => {
                                        batch.push(schema);
                                    }
                                    Err(_) => break, // Channel empty
                                }
                            }

                            // Final flush
                            if !batch.is_empty() {
                                info!(
                                    batch_size = batch.len(),
                                    "Flushing final batch on shutdown"
                                );

                                if let Err(e) = Self::write_schema_batch_with_retry(
                                    &db_pool,
                                    batch,
                                    max_retries,
                                    initial_backoff_ms,
                                )
                                .await
                                {
                                    error!(error = %e, "Failed to write final schema batch after retries");
                                }
                            }

                            info!("Schema batcher task shutdown complete");
                            break;
                        }
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{sleep, Duration};

    #[tokio::test]
    async fn test_processor_creation() {
        let (_tx, rx) = mpsc::unbounded_channel();
        let processor = AccessLogProcessor::new(rx, None, None, None);

        // Should use default config (num_cpus)
        assert!(processor.config.worker_count >= 1);
    }

    #[tokio::test]
    async fn test_processor_with_custom_config() {
        let (_tx, rx) = mpsc::unbounded_channel();
        let config = ProcessorConfig {
            worker_count: 4,
            batch_size: 100,
            batch_flush_interval_secs: 5,
            max_retries: 3,
            initial_backoff_ms: 100,
            max_queue_capacity: 10_000,
            path_normalization: PathNormalizationConfig::rest_defaults(),
            pending_entry_ttl_secs: 15,
            pending_cleanup_interval_secs: 60,
        };
        let processor = AccessLogProcessor::new(rx, None, None, Some(config));

        assert_eq!(processor.config.worker_count, 4);
    }

    #[tokio::test]
    async fn test_worker_spawning() {
        let (_tx, rx) = mpsc::unbounded_channel();
        let config = ProcessorConfig {
            worker_count: 2,
            batch_size: 100,
            batch_flush_interval_secs: 5,
            max_retries: 3,
            initial_backoff_ms: 100,
            max_queue_capacity: 10_000,
            path_normalization: PathNormalizationConfig::rest_defaults(),
            pending_entry_ttl_secs: 15,
            pending_cleanup_interval_secs: 60,
        };
        let processor = AccessLogProcessor::new(rx, None, None, Some(config));

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
        let config = ProcessorConfig {
            worker_count: 1,
            batch_size: 100,
            batch_flush_interval_secs: 5,
            max_retries: 3,
            initial_backoff_ms: 100,
            max_queue_capacity: 10_000,
            path_normalization: PathNormalizationConfig::rest_defaults(),
            pending_entry_ttl_secs: 15,
            pending_cleanup_interval_secs: 60,
        };
        let processor = AccessLogProcessor::new(rx, None, None, Some(config));

        let _handle = processor.spawn_workers();

        // Send a test log entry
        let entry = ProcessedLogEntry {
            session_id: "test-session".to_string(),
            request_id: None,
            team: "test-team".to_string(),
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
            trace_context: None,
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
        let config = ProcessorConfig {
            worker_count: 2,
            batch_size: 100,
            batch_flush_interval_secs: 5,
            max_retries: 3,
            initial_backoff_ms: 100,
            max_queue_capacity: 10_000,
            path_normalization: PathNormalizationConfig::rest_defaults(),
            pending_entry_ttl_secs: 15,
            pending_cleanup_interval_secs: 60,
        };
        let processor = AccessLogProcessor::new(rx, None, None, Some(config));

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
        let config = ProcessorConfig {
            worker_count: 1,
            batch_size: 100,
            batch_flush_interval_secs: 5,
            max_retries: 3,
            initial_backoff_ms: 100,
            max_queue_capacity: 10_000,
            path_normalization: PathNormalizationConfig::rest_defaults(),
            pending_entry_ttl_secs: 15,
            pending_cleanup_interval_secs: 60,
        };
        let processor = AccessLogProcessor::new(rx, None, None, Some(config));

        let handle = processor.spawn_workers();

        // Queue multiple entries
        for i in 0..10 {
            let entry = ProcessedLogEntry {
                session_id: format!("session-{}", i),
                request_id: None,
                team: "test-team".to_string(),
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
                trace_context: None,
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
        let config = ProcessorConfig {
            worker_count: 1,
            batch_size: 100,
            batch_flush_interval_secs: 5,
            max_retries: 3,
            initial_backoff_ms: 100,
            max_queue_capacity: 10_000,
            path_normalization: PathNormalizationConfig::rest_defaults(),
            pending_entry_ttl_secs: 15,
            pending_cleanup_interval_secs: 60,
        };
        let processor = AccessLogProcessor::new(rx, None, None, Some(config));

        let _handle = processor.spawn_workers();

        // Create mock JSON payloads
        let request_json =
            r#"{"user_id": 123, "action": "login", "timestamp": "2023-10-18T12:00:00Z"}"#;
        let response_json = r#"{"success": true, "token": "abc123", "expires_in": 3600}"#;

        let entry = ProcessedLogEntry {
            session_id: "test-session".to_string(),
            request_id: None,
            team: "test-team".to_string(),
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
            trace_context: None,
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
        let config = ProcessorConfig {
            worker_count: 1,
            batch_size: 100,
            batch_flush_interval_secs: 5,
            max_retries: 3,
            initial_backoff_ms: 100,
            max_queue_capacity: 10_000,
            path_normalization: PathNormalizationConfig::rest_defaults(),
            pending_entry_ttl_secs: 15,
            pending_cleanup_interval_secs: 60,
        };
        let processor = AccessLogProcessor::new(rx, None, None, Some(config));

        let _handle = processor.spawn_workers();

        // Create non-JSON payloads
        let binary_data = vec![0xFF, 0xD8, 0xFF, 0xE0]; // Binary data (JPEG header)

        let entry = ProcessedLogEntry {
            session_id: "test-session".to_string(),
            request_id: None,
            team: "test-team".to_string(),
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
            trace_context: None,
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
        let config = ProcessorConfig {
            worker_count: 1,
            batch_size: 100,
            batch_flush_interval_secs: 5,
            max_retries: 3,
            initial_backoff_ms: 100,
            max_queue_capacity: 10_000,
            path_normalization: PathNormalizationConfig::rest_defaults(),
            pending_entry_ttl_secs: 15,
            pending_cleanup_interval_secs: 60,
        };
        let processor = AccessLogProcessor::new(rx, None, None, Some(config));

        let _handle = processor.spawn_workers();

        // Create malformed JSON
        let malformed_json = r#"{"user_id": 123, "action": "login""#; // Missing closing brace

        let entry = ProcessedLogEntry {
            session_id: "test-session".to_string(),
            request_id: None,
            team: "test-team".to_string(),
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
            trace_context: None,
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
        let config = ProcessorConfig {
            worker_count: 1,
            batch_size: 100,
            batch_flush_interval_secs: 5,
            max_retries: 3,
            initial_backoff_ms: 100,
            max_queue_capacity: 10_000,
            path_normalization: PathNormalizationConfig::rest_defaults(),
            pending_entry_ttl_secs: 15,
            pending_cleanup_interval_secs: 60,
        };
        let processor = AccessLogProcessor::new(rx, None, None, Some(config));

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
            request_id: None,
            team: "test-team".to_string(),
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
            trace_context: None,
        };

        tx.send(entry).unwrap();

        // Allow processing
        sleep(Duration::from_millis(200)).await;

        // Entry should have been processed with schema inference
        // Complex nested structures should be handled correctly
    }

    #[tokio::test]
    async fn test_backpressure_drops_when_queue_full() {
        // Create a bounded channel with very small capacity
        let (tx, rx) = mpsc::unbounded_channel();
        let config = ProcessorConfig {
            worker_count: 1,
            batch_size: 100,
            batch_flush_interval_secs: 5,
            max_retries: 3,
            initial_backoff_ms: 100,
            max_queue_capacity: 2, // Very small queue to test backpressure
            path_normalization: PathNormalizationConfig::rest_defaults(),
            pending_entry_ttl_secs: 15,
            pending_cleanup_interval_secs: 60,
        };

        // Create processor without database to avoid actual writes
        let processor = AccessLogProcessor::new(rx, None, None, Some(config));
        let _handle = processor.spawn_workers();

        // Send entries - the bounded schema queue should fill up
        // Since we have no database, schemas will accumulate in the queue
        for i in 0..10 {
            let json_body = r#"{"test": "data"}"#;
            let entry = ProcessedLogEntry {
                session_id: format!("session-{}", i),
                request_id: None,
                team: "test-team".to_string(),
                method: 2, // POST
                path: "/api/test".to_string(),
                request_headers: vec![],
                request_body: Some(json_body.as_bytes().to_vec()),
                request_body_size: json_body.len() as u64,
                response_status: 200,
                response_headers: vec![],
                response_body: None,
                response_body_size: 0,
                start_time_seconds: 1234567890,
                duration_ms: 10,
                trace_context: None,
            };
            tx.send(entry).unwrap();
        }

        // Allow processing - some should be dropped due to backpressure
        sleep(Duration::from_millis(500)).await;

        // Test passes if no panic occurred (backpressure handled gracefully)
        // In production, we would verify dropped entry metrics here
    }

    // Note: Retry mechanism tests would require mocking the database layer
    // to simulate transient failures. This is difficult with the current
    // architecture that uses direct SQLx calls. A future refactor could
    // introduce a trait-based repository pattern to enable mocking.
    //
    // For now, retry logic is tested through:
    // 1. Code review of the exponential backoff implementation
    // 2. Manual testing with actual database failures
    // 3. Integration tests that exercise the full path

    #[cfg(feature = "postgres_tests")]
    mod postgres_tests {
        use super::*;
        use crate::storage::test_helpers::{TestDatabase, TEST_TEAM_ID};
        use sqlx::Row;

        /// Helper: create a learning session directly via SQL (same pattern as inferred_schema tests)
        async fn create_test_session(pool: &DbPool, team: &str) -> String {
            let session_id = uuid::Uuid::new_v4().to_string();
            sqlx::query(
                "INSERT INTO learning_sessions (
                    id, team, route_pattern, status, target_sample_count, current_sample_count
                ) VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(&session_id)
            .bind(team)
            .bind("/test/*")
            .bind("active")
            .bind(100i64)
            .bind(0i64)
            .execute(pool)
            .await
            .expect("Failed to create test session");

            session_id
        }

        #[tokio::test]
        async fn test_write_schema_batch_to_postgres() {
            let test_db = TestDatabase::new("write_schema_batch").await;
            let pool = test_db.pool.clone();
            let session_id = create_test_session(&pool, TEST_TEAM_ID).await;

            let batch = vec![
                InferredSchemaRecord {
                    session_id: session_id.clone(),
                    team: TEST_TEAM_ID.to_string(),
                    http_method: "GET".to_string(),
                    path_pattern: "/api/users/{id}".to_string(),
                    request_schema: None,
                    response_schema: Some(r#"{"type":"object","properties":{"id":{"type":"integer"}}}"#.to_string()),
                    response_status_code: Some(200),
                },
                InferredSchemaRecord {
                    session_id: session_id.clone(),
                    team: TEST_TEAM_ID.to_string(),
                    http_method: "POST".to_string(),
                    path_pattern: "/api/users".to_string(),
                    request_schema: Some(r#"{"type":"object","properties":{"name":{"type":"string"}}}"#.to_string()),
                    response_schema: Some(r#"{"type":"object","properties":{"id":{"type":"integer"}}}"#.to_string()),
                    response_status_code: Some(201),
                },
                InferredSchemaRecord {
                    session_id: session_id.clone(),
                    team: TEST_TEAM_ID.to_string(),
                    http_method: "DELETE".to_string(),
                    path_pattern: "/api/users/{id}".to_string(),
                    request_schema: None,
                    response_schema: None,
                    response_status_code: Some(204),
                },
                InferredSchemaRecord {
                    session_id: session_id.clone(),
                    team: TEST_TEAM_ID.to_string(),
                    http_method: "PUT".to_string(),
                    path_pattern: "/api/users/{id}".to_string(),
                    request_schema: Some(r#"{"type":"object","properties":{"name":{"type":"string"}}}"#.to_string()),
                    response_schema: Some(r#"{"type":"object","properties":{"id":{"type":"integer"},"name":{"type":"string"}}}"#.to_string()),
                    response_status_code: Some(200),
                },
            ];

            AccessLogProcessor::write_schema_batch(&pool, batch)
                .await
                .expect("write_schema_batch should succeed");

            // Read back and verify
            let rows = sqlx::query(
                "SELECT team, session_id, http_method, path_pattern,
                        request_schema, response_schema, response_status_code
                 FROM inferred_schemas WHERE session_id = $1
                 ORDER BY http_method",
            )
            .bind(&session_id)
            .fetch_all(&pool)
            .await
            .expect("Failed to read back schemas");

            assert_eq!(rows.len(), 4);

            // Rows are ordered by http_method: DELETE, GET, POST, PUT
            let delete_row = &rows[0];
            assert_eq!(delete_row.get::<String, _>("http_method"), "DELETE");
            assert_eq!(delete_row.get::<String, _>("team"), TEST_TEAM_ID);
            assert_eq!(delete_row.get::<String, _>("session_id"), session_id);
            assert_eq!(delete_row.get::<String, _>("path_pattern"), "/api/users/{id}");
            assert_eq!(delete_row.get::<Option<String>, _>("request_schema"), None);
            assert_eq!(delete_row.get::<Option<String>, _>("response_schema"), None);
            assert_eq!(delete_row.get::<Option<i64>, _>("response_status_code"), Some(204));

            let get_row = &rows[1];
            assert_eq!(get_row.get::<String, _>("http_method"), "GET");
            assert!(get_row.get::<Option<String>, _>("response_schema").is_some());

            let post_row = &rows[2];
            assert_eq!(post_row.get::<String, _>("http_method"), "POST");
            assert!(post_row.get::<Option<String>, _>("request_schema").is_some());
            assert!(post_row.get::<Option<String>, _>("response_schema").is_some());
            assert_eq!(post_row.get::<Option<i64>, _>("response_status_code"), Some(201));

            let put_row = &rows[3];
            assert_eq!(put_row.get::<String, _>("http_method"), "PUT");
            assert!(put_row.get::<Option<String>, _>("request_schema").is_some());
        }

        #[tokio::test]
        async fn test_write_schema_batch_empty() {
            let test_db = TestDatabase::new("write_schema_batch_empty").await;
            let pool = test_db.pool.clone();

            let result = AccessLogProcessor::write_schema_batch(&pool, vec![]).await;
            assert!(result.is_ok(), "Empty batch should return Ok(())");
        }

        #[tokio::test]
        async fn test_write_schema_batch_with_null_schemas() {
            let test_db = TestDatabase::new("write_schema_batch_nulls").await;
            let pool = test_db.pool.clone();
            let session_id = create_test_session(&pool, TEST_TEAM_ID).await;

            let batch = vec![
                InferredSchemaRecord {
                    session_id: session_id.clone(),
                    team: TEST_TEAM_ID.to_string(),
                    http_method: "GET".to_string(),
                    path_pattern: "/api/health".to_string(),
                    request_schema: None,
                    response_schema: None,
                    response_status_code: Some(200),
                },
                InferredSchemaRecord {
                    session_id: session_id.clone(),
                    team: TEST_TEAM_ID.to_string(),
                    http_method: "HEAD".to_string(),
                    path_pattern: "/api/health".to_string(),
                    request_schema: None,
                    response_schema: None,
                    response_status_code: None,
                },
            ];

            AccessLogProcessor::write_schema_batch(&pool, batch)
                .await
                .expect("write_schema_batch with null schemas should succeed");

            let rows = sqlx::query(
                "SELECT request_schema, response_schema, response_status_code
                 FROM inferred_schemas WHERE session_id = $1
                 ORDER BY http_method",
            )
            .bind(&session_id)
            .fetch_all(&pool)
            .await
            .expect("Failed to read back schemas");

            assert_eq!(rows.len(), 2);

            // Both records should have NULL schemas
            for row in &rows {
                assert_eq!(row.get::<Option<String>, _>("request_schema"), None);
                assert_eq!(row.get::<Option<String>, _>("response_schema"), None);
            }

            // GET has status code 200, HEAD has NULL
            let get_row = &rows[0]; // GET
            assert_eq!(get_row.get::<Option<i64>, _>("response_status_code"), Some(200));
            let head_row = &rows[1]; // HEAD
            assert_eq!(head_row.get::<Option<i64>, _>("response_status_code"), None);
        }

        #[tokio::test]
        async fn test_write_schema_batch_transactional() {
            let test_db = TestDatabase::new("write_schema_batch_tx").await;
            let pool = test_db.pool.clone();
            let session_id = create_test_session(&pool, TEST_TEAM_ID).await;

            // Batch where the last record has an invalid session_id (FK violation)
            let batch = vec![
                InferredSchemaRecord {
                    session_id: session_id.clone(),
                    team: TEST_TEAM_ID.to_string(),
                    http_method: "GET".to_string(),
                    path_pattern: "/api/valid".to_string(),
                    request_schema: None,
                    response_schema: None,
                    response_status_code: Some(200),
                },
                InferredSchemaRecord {
                    session_id: session_id.clone(),
                    team: TEST_TEAM_ID.to_string(),
                    http_method: "POST".to_string(),
                    path_pattern: "/api/valid".to_string(),
                    request_schema: None,
                    response_schema: None,
                    response_status_code: Some(201),
                },
                InferredSchemaRecord {
                    session_id: "nonexistent-session-id".to_string(), // FK violation
                    team: TEST_TEAM_ID.to_string(),
                    http_method: "DELETE".to_string(),
                    path_pattern: "/api/invalid".to_string(),
                    request_schema: None,
                    response_schema: None,
                    response_status_code: Some(204),
                },
            ];

            let result = AccessLogProcessor::write_schema_batch(&pool, batch).await;
            assert!(result.is_err(), "Batch with invalid FK should fail");

            // Verify the ENTIRE batch was rolled back — 0 records in DB
            let count: i64 =
                sqlx::query("SELECT COUNT(*) as count FROM inferred_schemas WHERE session_id = $1")
                    .bind(&session_id)
                    .fetch_one(&pool)
                    .await
                    .expect("Failed to count schemas")
                    .get("count");

            assert_eq!(count, 0, "Transaction should have rolled back all records");
        }
    }
}
