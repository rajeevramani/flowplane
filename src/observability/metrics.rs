//! # Metrics Collection
//!
//! Provides Prometheus metrics collection for the control plane.

use crate::config::ObservabilityConfig;
use crate::errors::{FlowplaneError, Result};
use ::tracing::{info, warn};
use metrics::{
    counter, describe_counter, describe_gauge, describe_histogram, gauge, histogram, Unit,
};
use metrics_exporter_prometheus::PrometheusBuilder;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Metrics recorder that tracks application metrics
#[derive(Debug, Clone, Default)]
pub struct MetricsRecorder;

impl MetricsRecorder {
    /// Create a new metrics recorder instance
    pub fn new() -> Self {
        Self
    }

    /// Record an HTTP request
    pub fn record_http_request(&self, method: &str, path: &str, status: u16, duration: f64) {
        counter!("http_requests_total").increment(1);
        histogram!("http_request_duration_seconds").record(duration);
        counter!("http_responses_total").increment(1);

        let request_labels = [("method", method.to_string()), ("path", path.to_string())];
        counter!("http_requests_total", &request_labels).increment(1);

        let status_label = [("status", status.to_string())];
        counter!("http_responses_total", &status_label).increment(1);
    }

    /// Record an xDS stream connection event
    pub fn record_xds_connection(&self, node_id: &str, connected: bool) {
        let labels = [("node_id", node_id.to_string())];
        if connected {
            gauge!("xds_connections_total", &labels).increment(1.0);
        } else {
            gauge!("xds_connections_total", &labels).decrement(1.0);
        }
    }

    /// Record an xDS request/response outcome
    pub fn record_xds_request(&self, type_url: &str, node_id: &str, success: bool) {
        let request_labels = [("type_url", type_url.to_string()), ("node_id", node_id.to_string())];
        counter!("xds_requests_total", &request_labels).increment(1);

        let status_label = if success { "success" } else { "error" };
        let response_labels =
            [("type_url", type_url.to_string()), ("status", status_label.to_string())];
        counter!("xds_responses_total", &response_labels).increment(1);
    }

    /// Record xDS stream duration in seconds
    pub fn record_xds_stream_duration(&self, node_id: &str, duration: f64) {
        let labels = [("node_id", node_id.to_string())];
        histogram!("xds_stream_duration_seconds", &labels).record(duration);
    }

    /// Record database activity with execution timing
    pub fn record_db_query(&self, operation: &str, table: &str, duration: f64, success: bool) {
        let op_table_labels = [("operation", operation.to_string()), ("table", table.to_string())];
        counter!("db_queries_total", &op_table_labels).increment(1);

        let status = if success { "success" } else { "error" };
        let status_labels = [("operation", operation.to_string()), ("status", status.to_string())];
        counter!("db_queries_total", &status_labels).increment(1);

        let duration_labels = [("operation", operation.to_string())];
        histogram!("db_query_duration_seconds", &duration_labels).record(duration);
    }

    /// Update database connection gauge
    pub fn update_db_connections(&self, active: u32) {
        gauge!("db_connections_active").set(active as f64);
    }

    /// Record configuration update activity
    pub fn record_config_update(&self, resource_type: &str, operation: &str) {
        let labels =
            [("resource_type", resource_type.to_string()), ("operation", operation.to_string())];
        counter!("config_updates_total", &labels).increment(1);
    }

    /// Update configuration object gauge
    pub fn update_config_objects(&self, resource_type: &str, count: u32) {
        let labels = [("resource_type", resource_type.to_string())];
        gauge!("config_objects_total", &labels).set(count as f64);
    }

    /// Update system uptime gauge
    pub fn update_uptime(&self, uptime_seconds: f64) {
        gauge!("system_uptime_seconds").set(uptime_seconds);
    }

    /// Update memory usage gauge
    pub fn update_memory_usage(&self, bytes: u64) {
        gauge!("memory_usage_bytes").set(bytes as f64);
    }

    /// Record token creation event
    pub fn record_token_created(&self, scope_count: usize) {
        counter!("auth_tokens_created_total").increment(1);
        let labels = [("scope_count", scope_count.to_string())];
        counter!("auth_tokens_created_total", &labels).increment(1);
    }

    /// Record token revocation event
    pub fn record_token_revoked(&self) {
        counter!("auth_tokens_revoked_total").increment(1);
    }

    /// Record token rotation event
    pub fn record_token_rotated(&self) {
        counter!("auth_tokens_rotated_total").increment(1);
    }

    /// Record authentication attempt outcome
    pub fn record_authentication(&self, status: &str) {
        counter!("auth_authentications_total").increment(1);
        let labels = [("status", status.to_string())];
        counter!("auth_authentications_total", &labels).increment(1);
    }

    /// Update gauge tracking active personal access tokens
    pub fn set_active_tokens(&self, count: usize) {
        gauge!("auth_tokens_active_total", "state" => "active").set(count as f64);
    }

    /// Update team-scoped resource count gauge
    pub fn update_team_resource_count(&self, resource_type: &str, team: &str, count: usize) {
        let labels = [("resource_type", resource_type.to_string()), ("team", team.to_string())];
        gauge!("xds_team_resources_total", &labels).set(count as f64);
    }

    /// Record cross-team access attempt
    pub fn record_cross_team_access_attempt(
        &self,
        from_team: &str,
        to_team: &str,
        resource_type: &str,
    ) {
        let labels = [
            ("from_team", from_team.to_string()),
            ("to_team", to_team.to_string()),
            ("resource_type", resource_type.to_string()),
        ];
        counter!("auth_cross_team_access_attempts_total", &labels).increment(1);
    }

    /// Record team-scoped xDS connection event
    pub fn record_team_xds_connection(&self, team: &str, connected: bool) {
        let labels = [("team", team.to_string())];
        if connected {
            gauge!("xds_team_connections", &labels).increment(1.0);
        } else {
            gauge!("xds_team_connections", &labels).decrement(1.0);
        }
    }

    /// Record access log message received
    pub fn record_access_log_message(&self, entry_count: usize) {
        counter!("access_log_messages_total").increment(1);
        counter!("access_log_entries_total").increment(entry_count as u64);
    }

    /// Record access log processing latency
    pub fn record_access_log_latency(&self, duration: f64) {
        histogram!("access_log_processing_duration_seconds").record(duration);
    }

    /// Record access log filtering result
    pub fn record_access_log_filter(&self, matched: bool) {
        let status = if matched { "matched" } else { "filtered" };
        let labels = [("status", status.to_string())];
        counter!("access_log_filter_results_total", &labels).increment(1);
    }

    /// Record queued access log entry
    pub fn record_access_log_queued(&self, session_id: &str) {
        counter!("access_log_entries_queued_total").increment(1);
        let labels = [("session_id", session_id.to_string())];
        counter!("access_log_entries_queued_total", &labels).increment(1);
    }

    /// Update active learning sessions gauge
    pub fn update_active_learning_sessions(&self, count: usize) {
        gauge!("access_log_learning_sessions_active").set(count as f64);
    }

    /// Record schema inference from access log
    pub fn record_schema_inferred(&self, schema_type: &str, success: bool) {
        let status = if success { "success" } else { "error" };
        let labels = [("schema_type", schema_type.to_string()), ("status", status.to_string())];
        counter!("access_log_schemas_inferred_total", &labels).increment(1);
    }

    /// Record schema batch write operation
    pub fn record_schema_batch_write(&self, batch_size: usize, success: bool, retry_count: usize) {
        counter!("access_log_schema_batches_written_total").increment(1);
        histogram!("access_log_schema_batch_size").record(batch_size as f64);

        let status = if success { "success" } else { "error" };
        let labels = [("status", status.to_string())];
        counter!("access_log_schema_batches_written_total", &labels).increment(1);

        if retry_count > 0 {
            counter!("access_log_schema_batch_retries_total").increment(retry_count as u64);
            histogram!("access_log_schema_batch_retry_count").record(retry_count as f64);
        }
    }

    /// Record schema dropped due to backpressure
    pub fn record_schema_dropped(&self, schema_type: &str) {
        counter!("access_log_schemas_dropped_total").increment(1);
        let labels = [("schema_type", schema_type.to_string())];
        counter!("access_log_schemas_dropped_total", &labels).increment(1);
    }

    /// Update processor queue depth gauge
    pub fn update_processor_queue_depth(&self, queue_type: &str, depth: usize) {
        let labels = [("queue_type", queue_type.to_string())];
        gauge!("access_log_processor_queue_depth", &labels).set(depth as f64);
    }

    /// Update active processor workers gauge
    pub fn update_processor_workers(&self, count: usize) {
        gauge!("access_log_processor_workers_active").set(count as f64);
    }

    /// Record processor entry processing duration
    pub fn record_processor_entry_duration(&self, duration: f64) {
        histogram!("access_log_processor_entry_duration_seconds").record(duration);
    }

    /// Register baseline auth metrics so Prometheus exports appear before events occur.
    pub fn register_auth_metrics(&self) {
        describe_counter!(
            "auth_tokens_created_total",
            Unit::Count,
            "Number of personal access tokens created"
        );
        describe_counter!(
            "auth_tokens_revoked_total",
            Unit::Count,
            "Number of personal access tokens revoked"
        );
        describe_counter!(
            "auth_tokens_rotated_total",
            Unit::Count,
            "Number of personal access tokens rotated"
        );
        describe_counter!(
            "auth_authentications_total",
            Unit::Count,
            "Authentication attempts grouped by outcome"
        );
        describe_gauge!(
            "auth_tokens_active_total",
            Unit::Count,
            "Gauge tracking active personal access tokens"
        );

        counter!("auth_tokens_created_total").absolute(0);
        counter!("auth_tokens_revoked_total").absolute(0);
        counter!("auth_tokens_rotated_total").absolute(0);
        gauge!("auth_tokens_active_total", "state" => "active").set(0.0);

        const STATUSES: &[&str] = &[
            "success",
            "missing_bearer",
            "malformed",
            "not_found",
            "inactive",
            "expired",
            "invalid_secret",
            "error",
        ];

        for status in STATUSES {
            counter!("auth_authentications_total", "status" => *status).absolute(0);
        }
    }

    /// Register team-based metrics for xDS resource distribution
    pub fn register_team_metrics(&self) {
        describe_gauge!(
            "xds_team_resources_total",
            Unit::Count,
            "Number of resources (clusters, routes, listeners) served per team"
        );
        describe_counter!(
            "auth_cross_team_access_attempts_total",
            Unit::Count,
            "Cross-team resource access attempts blocked by authorization"
        );
        describe_gauge!("xds_team_connections", Unit::Count, "Active xDS connections per team");
    }

    /// Register access log service metrics
    pub fn register_access_log_metrics(&self) {
        describe_counter!(
            "access_log_messages_total",
            Unit::Count,
            "Total number of access log messages received from Envoy"
        );
        describe_counter!(
            "access_log_entries_total",
            Unit::Count,
            "Total number of access log entries processed"
        );
        describe_histogram!(
            "access_log_processing_duration_seconds",
            Unit::Seconds,
            "Duration of access log message processing"
        );
        describe_counter!(
            "access_log_filter_results_total",
            Unit::Count,
            "Access log filtering results (matched vs filtered)"
        );
        describe_counter!(
            "access_log_entries_queued_total",
            Unit::Count,
            "Number of access log entries queued for background processing"
        );
        describe_gauge!(
            "access_log_learning_sessions_active",
            Unit::Count,
            "Number of active learning sessions"
        );

        // Initialize counters to zero
        counter!("access_log_messages_total").absolute(0);
        counter!("access_log_entries_total").absolute(0);
        counter!("access_log_filter_results_total", "status" => "matched").absolute(0);
        counter!("access_log_filter_results_total", "status" => "filtered").absolute(0);
        counter!("access_log_entries_queued_total").absolute(0);
        gauge!("access_log_learning_sessions_active").set(0.0);
    }

    /// Register access log processor metrics
    pub fn register_processor_metrics(&self) {
        describe_counter!(
            "access_log_schemas_inferred_total",
            Unit::Count,
            "Number of schemas inferred from access logs"
        );
        describe_counter!(
            "access_log_schema_batches_written_total",
            Unit::Count,
            "Number of schema batches written to database"
        );
        describe_histogram!(
            "access_log_schema_batch_size",
            Unit::Count,
            "Size of schema batches written"
        );
        describe_counter!(
            "access_log_schema_batch_retries_total",
            Unit::Count,
            "Total number of batch write retries"
        );
        describe_histogram!(
            "access_log_schema_batch_retry_count",
            Unit::Count,
            "Number of retries per batch write"
        );
        describe_counter!(
            "access_log_schemas_dropped_total",
            Unit::Count,
            "Number of schemas dropped due to backpressure"
        );
        describe_gauge!(
            "access_log_processor_queue_depth",
            Unit::Count,
            "Current depth of processor queues"
        );
        describe_gauge!(
            "access_log_processor_workers_active",
            Unit::Count,
            "Number of active processor workers"
        );
        describe_histogram!(
            "access_log_processor_entry_duration_seconds",
            Unit::Seconds,
            "Duration of processing a single log entry"
        );

        // Initialize counters to zero
        counter!("access_log_schemas_inferred_total", "schema_type" => "request", "status" => "success").absolute(0);
        counter!("access_log_schemas_inferred_total", "schema_type" => "request", "status" => "error").absolute(0);
        counter!("access_log_schemas_inferred_total", "schema_type" => "response", "status" => "success").absolute(0);
        counter!("access_log_schemas_inferred_total", "schema_type" => "response", "status" => "error").absolute(0);
        counter!("access_log_schema_batches_written_total", "status" => "success").absolute(0);
        counter!("access_log_schema_batches_written_total", "status" => "error").absolute(0);
        counter!("access_log_schema_batch_retries_total").absolute(0);
        counter!("access_log_schemas_dropped_total").absolute(0);
        gauge!("access_log_processor_queue_depth", "queue_type" => "log_entries").set(0.0);
        gauge!("access_log_processor_queue_depth", "queue_type" => "schemas").set(0.0);
        gauge!("access_log_processor_workers_active").set(0.0);
    }
}

/// Global metrics recorder instance
static METRICS: once_cell::sync::Lazy<Arc<RwLock<Option<MetricsRecorder>>>> =
    once_cell::sync::Lazy::new(|| Arc::new(RwLock::new(None)));

/// Initialize metrics collection and Prometheus exporter
pub async fn init_metrics(config: &ObservabilityConfig) -> Result<()> {
    if !config.enable_metrics {
        return Ok(());
    }

    let metrics_addr = match config.metrics_bind_address() {
        Some(addr) => addr,
        None => {
            warn!("Metrics disabled: no bind address configured");
            return Ok(());
        }
    };

    let socket_addr: SocketAddr = metrics_addr.parse().map_err(|e| {
        FlowplaneError::config(format!("Invalid metrics bind address '{}': {}", metrics_addr, e))
    })?;

    // Initialize Prometheus exporter
    let builder = PrometheusBuilder::new()
        .with_http_listener(socket_addr)
        .add_global_label("service", &config.service_name);

    builder.install().map_err(|e| {
        FlowplaneError::config(format!("Failed to initialize metrics exporter: {}", e))
    })?;

    // Create and store global metrics recorder
    let recorder = MetricsRecorder::new();
    {
        let mut metrics = METRICS.write().await;
        *metrics = Some(recorder.clone());
    }

    recorder.register_auth_metrics();
    recorder.register_team_metrics();
    recorder.register_access_log_metrics();
    recorder.register_processor_metrics();

    info!(
        metrics_addr = %metrics_addr,
        service_name = %config.service_name,
        "Metrics collection initialized"
    );

    Ok(())
}

/// Get the global metrics recorder
pub async fn get_metrics() -> Option<MetricsRecorder> {
    METRICS.read().await.clone()
}

/// Record an HTTP request using the global metrics recorder
pub async fn record_http_request(method: &str, path: &str, status: u16, duration: f64) {
    if let Some(metrics) = get_metrics().await {
        metrics.record_http_request(method, path, status, duration);
    }
}

/// Record an xDS operation using the global metrics recorder
pub async fn record_xds_operation(type_url: &str, node_id: &str, success: bool) {
    if let Some(metrics) = get_metrics().await {
        metrics.record_xds_request(type_url, node_id, success);
    }
}

/// Record a database operation using the global metrics recorder
pub async fn record_db_operation(operation: &str, table: &str, duration: f64, success: bool) {
    if let Some(metrics) = get_metrics().await {
        metrics.record_db_query(operation, table, duration, success);
    }
}

/// Record personal access token creation via the global recorder
pub async fn record_token_created(scope_count: usize) {
    if let Some(metrics) = get_metrics().await {
        metrics.record_token_created(scope_count);
    }
}

/// Record personal access token revocation via the global recorder
pub async fn record_token_revoked() {
    if let Some(metrics) = get_metrics().await {
        metrics.record_token_revoked();
    }
}

/// Record personal access token rotation via the global recorder
pub async fn record_token_rotated() {
    if let Some(metrics) = get_metrics().await {
        metrics.record_token_rotated();
    }
}

/// Record authentication attempt outcome via the global recorder
pub async fn record_authentication(status: &str) {
    if let Some(metrics) = get_metrics().await {
        metrics.record_authentication(status);
    }
}

/// Update the active personal access token gauge via the global recorder
pub async fn set_active_tokens(count: usize) {
    if let Some(metrics) = get_metrics().await {
        metrics.set_active_tokens(count);
    }
}

/// Update team-scoped resource count via the global recorder
pub async fn update_team_resource_count(resource_type: &str, team: &str, count: usize) {
    if let Some(metrics) = get_metrics().await {
        metrics.update_team_resource_count(resource_type, team, count);
    }
}

/// Record cross-team access attempt via the global recorder
pub async fn record_cross_team_access_attempt(from_team: &str, to_team: &str, resource_type: &str) {
    if let Some(metrics) = get_metrics().await {
        metrics.record_cross_team_access_attempt(from_team, to_team, resource_type);
    }
}

/// Record team-scoped xDS connection via the global recorder
pub async fn record_team_xds_connection(team: &str, connected: bool) {
    if let Some(metrics) = get_metrics().await {
        metrics.record_team_xds_connection(team, connected);
    }
}

/// Record access log message received via the global recorder
pub async fn record_access_log_message(entry_count: usize) {
    if let Some(metrics) = get_metrics().await {
        metrics.record_access_log_message(entry_count);
    }
}

/// Record access log processing latency via the global recorder
pub async fn record_access_log_latency(duration: f64) {
    if let Some(metrics) = get_metrics().await {
        metrics.record_access_log_latency(duration);
    }
}

/// Record access log filter result via the global recorder
pub async fn record_access_log_filter(matched: bool) {
    if let Some(metrics) = get_metrics().await {
        metrics.record_access_log_filter(matched);
    }
}

/// Record access log entry queued via the global recorder
pub async fn record_access_log_queued(session_id: &str) {
    if let Some(metrics) = get_metrics().await {
        metrics.record_access_log_queued(session_id);
    }
}

/// Update active learning sessions count via the global recorder
pub async fn update_active_learning_sessions(count: usize) {
    if let Some(metrics) = get_metrics().await {
        metrics.update_active_learning_sessions(count);
    }
}

/// Record schema inference via the global recorder
pub async fn record_schema_inferred(schema_type: &str, success: bool) {
    if let Some(metrics) = get_metrics().await {
        metrics.record_schema_inferred(schema_type, success);
    }
}

/// Record schema batch write via the global recorder
pub async fn record_schema_batch_write(batch_size: usize, success: bool, retry_count: usize) {
    if let Some(metrics) = get_metrics().await {
        metrics.record_schema_batch_write(batch_size, success, retry_count);
    }
}

/// Record schema dropped via the global recorder
pub async fn record_schema_dropped(schema_type: &str) {
    if let Some(metrics) = get_metrics().await {
        metrics.record_schema_dropped(schema_type);
    }
}

/// Update processor queue depth via the global recorder
pub async fn update_processor_queue_depth(queue_type: &str, depth: usize) {
    if let Some(metrics) = get_metrics().await {
        metrics.update_processor_queue_depth(queue_type, depth);
    }
}

/// Update processor workers via the global recorder
pub async fn update_processor_workers(count: usize) {
    if let Some(metrics) = get_metrics().await {
        metrics.update_processor_workers(count);
    }
}

/// Record processor entry duration via the global recorder
pub async fn record_processor_entry_duration(duration: f64) {
    if let Some(metrics) = get_metrics().await {
        metrics.record_processor_entry_duration(duration);
    }
}

/// Middleware for automatic HTTP metrics collection
pub struct MetricsMiddleware;

impl MetricsMiddleware {
    /// Create a new metrics middleware
    pub fn new() -> Self {
        Self
    }
}

impl Default for MetricsMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

/// System metrics collector that runs periodically
pub struct SystemMetricsCollector {
    start_time: std::time::Instant,
}

impl Default for SystemMetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemMetricsCollector {
    /// Create a new system metrics collector
    pub fn new() -> Self {
        Self { start_time: std::time::Instant::now() }
    }

    /// Start collecting system metrics periodically
    pub async fn start(&self, interval: std::time::Duration) {
        let mut ticker = tokio::time::interval(interval);
        let start_time = self.start_time;

        loop {
            ticker.tick().await;

            if let Some(metrics) = get_metrics().await {
                // Update uptime
                let uptime = start_time.elapsed().as_secs_f64();
                metrics.update_uptime(uptime);

                // Update memory usage (simple implementation)
                if let Ok(memory_info) = self.get_memory_usage() {
                    metrics.update_memory_usage(memory_info);
                }
            }
        }
    }

    /// Get current memory usage (placeholder implementation)
    fn get_memory_usage(&self) -> Result<u64> {
        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_recorder_creation() {
        let recorder = MetricsRecorder::new();
        recorder.record_http_request("GET", "/", 200, 0.5);
    }

    #[test]
    fn test_metrics_recording() {
        let recorder = MetricsRecorder::new();

        recorder.record_http_request("GET", "/api/clusters", 200, 0.123);
        recorder.record_http_request("POST", "/api/clusters", 201, 0.456);

        recorder.record_xds_connection("node-1", true);
        recorder.record_xds_request(
            "type.googleapis.com/envoy.config.cluster.v3.Cluster",
            "node-1",
            true,
        );
        recorder.record_xds_stream_duration("node-1", 120.5);

        recorder.record_db_query("SELECT", "clusters", 0.05, true);
        recorder.update_db_connections(15);

        recorder.record_config_update("cluster", "create");
        recorder.update_config_objects("cluster", 42);

        recorder.update_uptime(7200.0);
        recorder.update_memory_usage(1024 * 1024 * 128);
        recorder.set_active_tokens(5);

        // Test team-based metrics
        recorder.update_team_resource_count("cluster", "payments", 42);
        recorder.update_team_resource_count("route", "billing", 15);
        recorder.update_team_resource_count("listener", "platform", 3);
        recorder.record_cross_team_access_attempt("payments", "billing", "clusters");
        recorder.record_team_xds_connection("payments", true);
        recorder.record_team_xds_connection("billing", true);
        recorder.record_team_xds_connection("payments", false);
    }

    #[tokio::test]
    async fn test_init_metrics_disabled() {
        let config = ObservabilityConfig { enable_metrics: false, ..Default::default() };

        let result = init_metrics(&config).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_init_metrics_no_port() {
        let config =
            ObservabilityConfig { enable_metrics: true, metrics_port: 0, ..Default::default() };

        let result = init_metrics(&config).await;
        assert!(result.is_ok());
    }
}
