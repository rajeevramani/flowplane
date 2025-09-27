//! # Metrics Collection
//!
//! Provides Prometheus metrics collection for the control plane.

use crate::config::ObservabilityConfig;
use crate::errors::{FlowplaneError, Result};
use ::tracing::{info, warn};
use metrics::{counter, describe_counter, describe_gauge, gauge, histogram, Unit};
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
