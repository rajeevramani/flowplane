//! # Metrics Collection
//!
//! Provides Prometheus metrics collection for the control plane.

use crate::config::ObservabilityConfig;
use crate::errors::{FlowplaneError, Result};
use metrics::{counter, gauge, histogram, Counter, Gauge, Histogram};
use metrics_exporter_prometheus::PrometheusBuilder;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Metrics recorder that tracks application metrics
#[derive(Debug, Clone)]
pub struct MetricsRecorder {
    // HTTP metrics
    pub http_requests_total: Counter,
    pub http_request_duration: Histogram,
    pub http_responses_total: Counter,

    // xDS metrics
    pub xds_connections_total: Gauge,
    pub xds_requests_total: Counter,
    pub xds_responses_total: Counter,
    pub xds_stream_duration: Histogram,

    // Database metrics
    pub db_connections_active: Gauge,
    pub db_queries_total: Counter,
    pub db_query_duration: Histogram,

    // Configuration metrics
    pub config_updates_total: Counter,
    pub config_objects_total: Gauge,

    // System metrics
    pub system_uptime: Gauge,
    pub memory_usage: Gauge,
}

impl MetricsRecorder {
    /// Create a new metrics recorder with initialized metrics
    pub fn new() -> Self {
        Self {
            // HTTP metrics
            http_requests_total: counter!("http_requests_total"),
            http_request_duration: histogram!("http_request_duration_seconds"),
            http_responses_total: counter!("http_responses_total"),

            // xDS metrics
            xds_connections_total: gauge!("xds_connections_total"),
            xds_requests_total: counter!("xds_requests_total"),
            xds_responses_total: counter!("xds_responses_total"),
            xds_stream_duration: histogram!("xds_stream_duration_seconds"),

            // Database metrics
            db_connections_active: gauge!("db_connections_active"),
            db_queries_total: counter!("db_queries_total"),
            db_query_duration: histogram!("db_query_duration_seconds"),

            // Configuration metrics
            config_updates_total: counter!("config_updates_total"),
            config_objects_total: gauge!("config_objects_total"),

            // System metrics
            system_uptime: gauge!("system_uptime_seconds"),
            memory_usage: gauge!("memory_usage_bytes"),
        }
    }

    /// Record an HTTP request
    pub fn record_http_request(&self, method: &str, path: &str, status: u16, duration: f64) {
        self.http_requests_total.increment(1);
        self.http_request_duration.record(duration);
        self.http_responses_total.increment(1);

        // Record with labels using the metrics crate's label syntax
        counter!("http_requests_total", "method" => method, "path" => path).increment(1);
        counter!("http_responses_total", "status" => status.to_string()).increment(1);
    }

    /// Record an xDS stream connection
    pub fn record_xds_connection(&self, node_id: &str, connected: bool) {
        if connected {
            self.xds_connections_total.increment(1.0);
            gauge!("xds_connections_total", "node_id" => node_id).increment(1.0);
        } else {
            self.xds_connections_total.decrement(1.0);
            gauge!("xds_connections_total", "node_id" => node_id).decrement(1.0);
        }
    }

    /// Record an xDS request/response
    pub fn record_xds_request(&self, type_url: &str, node_id: &str, success: bool) {
        self.xds_requests_total.increment(1);
        counter!("xds_requests_total", "type_url" => type_url, "node_id" => node_id).increment(1);

        if success {
            self.xds_responses_total.increment(1);
            counter!("xds_responses_total", "type_url" => type_url, "status" => "success").increment(1);
        } else {
            counter!("xds_responses_total", "type_url" => type_url, "status" => "error").increment(1);
        }
    }

    /// Record xDS stream duration
    pub fn record_xds_stream_duration(&self, node_id: &str, duration: f64) {
        self.xds_stream_duration.record(duration);
        histogram!("xds_stream_duration_seconds", "node_id" => node_id).record(duration);
    }

    /// Record database activity
    pub fn record_db_query(&self, operation: &str, table: &str, duration: f64, success: bool) {
        self.db_queries_total.increment(1);
        self.db_query_duration.record(duration);

        counter!("db_queries_total", "operation" => operation, "table" => table).increment(1);
        let status = if success { "success" } else { "error" };
        counter!("db_queries_total", "operation" => operation, "status" => status).increment(1);
    }

    /// Update database connections count
    pub fn update_db_connections(&self, active: u32) {
        self.db_connections_active.set(active as f64);
    }

    /// Record configuration update
    pub fn record_config_update(&self, resource_type: &str, operation: &str) {
        self.config_updates_total.increment(1);
        counter!("config_updates_total", "resource_type" => resource_type, "operation" => operation)
            .increment(1);
    }

    /// Update configuration objects count
    pub fn update_config_objects(&self, resource_type: &str, count: u32) {
        gauge!("config_objects_total", "resource_type" => resource_type).set(count as f64);
    }

    /// Update system uptime
    pub fn update_uptime(&self, uptime_seconds: f64) {
        self.system_uptime.set(uptime_seconds);
    }

    /// Update memory usage
    pub fn update_memory_usage(&self, bytes: u64) {
        self.memory_usage.set(bytes as f64);
    }
}

impl Default for MetricsRecorder {
    fn default() -> Self {
        Self::new()
    }
}

/// Global metrics recorder instance
static METRICS: once_cell::sync::Lazy<Arc<RwLock<Option<MetricsRecorder>>>> =
    once_cell::sync::Lazy::new(|| Arc::new(RwLock::new(None)));

/// Initialize metrics collection and Prometheus exporter
pub fn init_metrics(config: &ObservabilityConfig) -> Result<()> {
    if !config.enable_metrics {
        return Ok(());
    }

    let metrics_addr = match config.metrics_bind_address() {
        Some(addr) => addr,
        None => {
            tracing::warn!("Metrics disabled: no bind address configured");
            return Ok(());
        }
    };

    let socket_addr: SocketAddr = metrics_addr
        .parse()
        .map_err(|e| FlowplaneError::config(format!("Invalid metrics bind address '{}': {}", metrics_addr, e)))?;

    // Initialize Prometheus exporter
    let builder = PrometheusBuilder::new()
        .with_http_listener(socket_addr)
        .add_global_label("service", &config.service_name);

    builder
        .install()
        .map_err(|e| FlowplaneError::config(format!("Failed to initialize metrics exporter: {}", e)))?;

    // Create and store global metrics recorder
    let recorder = MetricsRecorder::new();
    tokio::spawn(async move {
        let mut metrics = METRICS.write().await;
        *metrics = Some(recorder);
    });

    tracing::info!(
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

/// Middleware for automatic HTTP metrics collection
pub struct MetricsMiddleware;

impl MetricsMiddleware {
    /// Create a new metrics middleware
    pub fn new() -> Self {
        Self
    }
}

/// System metrics collector that runs periodically
pub struct SystemMetricsCollector {
    start_time: std::time::Instant,
}

impl SystemMetricsCollector {
    /// Create a new system metrics collector
    pub fn new() -> Self {
        Self {
            start_time: std::time::Instant::now(),
        }
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
                // In a real implementation, you might use a crate like `sysinfo`
                if let Ok(memory_info) = self.get_memory_usage() {
                    metrics.update_memory_usage(memory_info);
                }
            }
        }
    }

    /// Get current memory usage (placeholder implementation)
    fn get_memory_usage(&self) -> Result<u64> {
        // This is a placeholder. In a real implementation, you would use
        // system APIs or crates like `sysinfo` to get actual memory usage.
        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_recorder_creation() {
        let recorder = MetricsRecorder::new();

        // Test that all metrics are initialized
        recorder.http_requests_total.increment(1);
        recorder.xds_connections_total.set(5.0);
        recorder.db_connections_active.set(10.0);
        recorder.config_updates_total.increment(1);
        recorder.system_uptime.set(3600.0);
    }

    #[test]
    fn test_metrics_recording() {
        let recorder = MetricsRecorder::new();

        // Test HTTP metrics
        recorder.record_http_request("GET", "/api/clusters", 200, 0.123);
        recorder.record_http_request("POST", "/api/clusters", 201, 0.456);

        // Test xDS metrics
        recorder.record_xds_connection("node-1", true);
        recorder.record_xds_request("type.googleapis.com/envoy.config.cluster.v3.Cluster", "node-1", true);
        recorder.record_xds_stream_duration("node-1", 120.5);

        // Test database metrics
        recorder.record_db_query("SELECT", "clusters", 0.05, true);
        recorder.update_db_connections(15);

        // Test configuration metrics
        recorder.record_config_update("cluster", "create");
        recorder.update_config_objects("cluster", 42);

        // Test system metrics
        recorder.update_uptime(7200.0);
        recorder.update_memory_usage(1024 * 1024 * 128); // 128MB
    }

    #[test]
    fn test_init_metrics_disabled() {
        let config = ObservabilityConfig {
            enable_metrics: false,
            ..Default::default()
        };

        let result = init_metrics(&config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_init_metrics_no_port() {
        let config = ObservabilityConfig {
            enable_metrics: true,
            metrics_port: 0,
            ..Default::default()
        };

        let result = init_metrics(&config);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_global_metrics_functions() {
        // These functions should not panic even if no metrics recorder is initialized
        record_http_request("GET", "/test", 200, 0.1).await;
        record_xds_operation("test.type", "node-1", true).await;
        record_db_operation("SELECT", "test_table", 0.05, true).await;
    }

    #[test]
    fn test_system_metrics_collector() {
        let collector = SystemMetricsCollector::new();
        assert!(collector.start_time.elapsed().as_millis() < 100); // Should be very recent
    }
}