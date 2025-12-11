//! Stats data source abstraction for Envoy metrics collection
//!
//! This module provides a trait-based abstraction for fetching stats from
//! different sources (Envoy admin API, Prometheus, etc.), enabling testability
//! and future extensibility.

use crate::domain::{
    ClusterStats, ConnectionMetrics, EnvoyHealthStatus, GaugeMetric, LatencyMetrics, ListenerStats,
    RequestMetrics, ResponseCodeMetrics, ServerStats, StatsSnapshot,
};
use crate::errors::Result;
use async_trait::async_trait;
use reqwest::Client;
use std::collections::HashMap;
use std::time::Duration;
use tracing::{debug, warn};

/// Trait for fetching stats from a data source
#[async_trait]
pub trait StatsDataSource: Send + Sync {
    /// Fetch a complete stats snapshot for a team
    async fn fetch_stats(&self, team_id: &str, admin_url: &str) -> Result<StatsSnapshot>;

    /// Check if the data source is reachable
    async fn health_check(&self, admin_url: &str) -> Result<bool>;
}

/// Configuration for the Envoy admin stats client
#[derive(Debug, Clone)]
pub struct EnvoyAdminConfig {
    /// Request timeout
    pub timeout: Duration,
    /// Connect timeout
    pub connect_timeout: Duration,
}

impl Default for EnvoyAdminConfig {
    fn default() -> Self {
        Self { timeout: Duration::from_secs(5), connect_timeout: Duration::from_secs(2) }
    }
}

/// Implementation of StatsDataSource that fetches from Envoy's admin API
pub struct EnvoyAdminStats {
    client: Client,
}

impl EnvoyAdminStats {
    /// Create a new EnvoyAdminStats client
    pub fn new(config: EnvoyAdminConfig) -> Result<Self> {
        let client = Client::builder()
            .timeout(config.timeout)
            .connect_timeout(config.connect_timeout)
            .build()
            .map_err(|e| {
                crate::errors::FlowplaneError::internal(format!(
                    "Failed to create HTTP client: {}",
                    e
                ))
            })?;

        Ok(Self { client })
    }

    /// Parse Envoy stats text format into a map of metric name -> value
    fn parse_stats_text(text: &str) -> HashMap<String, f64> {
        let mut metrics = HashMap::new();

        for line in text.lines() {
            // Skip empty lines and comments
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            // Format: "metric.name: value" or "metric.name: value (type)"
            if let Some(colon_pos) = line.find(':') {
                let name = line[..colon_pos].trim();
                let value_part = line[colon_pos + 1..].trim();

                // Handle "value (type)" format - extract just the value
                let value_str = if let Some(paren_pos) = value_part.find('(') {
                    value_part[..paren_pos].trim()
                } else {
                    value_part
                };

                // Parse the value - skip "No recorded values" and similar
                if let Ok(value) = value_str.parse::<f64>() {
                    metrics.insert(name.to_string(), value);
                }
            }
        }

        metrics
    }

    /// Extract cluster stats from parsed metrics
    fn extract_cluster_stats(metrics: &HashMap<String, f64>) -> Vec<ClusterStats> {
        // Collect unique cluster names
        let mut cluster_names: std::collections::HashSet<String> = std::collections::HashSet::new();

        for name in metrics.keys() {
            // Cluster metrics look like: cluster.{cluster_name}.{metric_name}
            if name.starts_with("cluster.") {
                let parts: Vec<&str> = name.splitn(3, '.').collect();
                if parts.len() >= 2 {
                    cluster_names.insert(parts[1].to_string());
                }
            }
        }

        cluster_names
            .into_iter()
            .map(|cluster_name| {
                let prefix = format!("cluster.{}.", cluster_name);

                let healthy_hosts =
                    metrics.get(&format!("{}membership_healthy", prefix)).copied().unwrap_or(0.0)
                        as u64;

                let total_hosts =
                    metrics.get(&format!("{}membership_total", prefix)).copied().unwrap_or(0.0)
                        as u64;

                let upstream_cx_active =
                    metrics.get(&format!("{}upstream_cx_active", prefix)).copied().unwrap_or(0.0)
                        as u64;

                let upstream_cx_total =
                    metrics.get(&format!("{}upstream_cx_total", prefix)).copied().unwrap_or(0.0)
                        as u64;

                let upstream_rq_active =
                    metrics.get(&format!("{}upstream_rq_active", prefix)).copied().unwrap_or(0.0)
                        as u64;

                let upstream_rq_total =
                    metrics.get(&format!("{}upstream_rq_total", prefix)).copied().unwrap_or(0.0)
                        as u64;

                let upstream_rq_pending = metrics
                    .get(&format!("{}upstream_rq_pending_total", prefix))
                    .copied()
                    .unwrap_or(0.0) as u64;

                // Circuit breaker triggered if we have overflow
                let cb_remaining = metrics
                    .get(&format!("{}circuit_breakers.default.remaining_cx", prefix))
                    .copied()
                    .unwrap_or(f64::MAX);
                let circuit_breaker_open = cb_remaining <= 0.0;

                let outlier_ejections = metrics
                    .get(&format!("{}outlier_detection.ejections_active", prefix))
                    .copied()
                    .unwrap_or(0.0) as u64;

                // Calculate success rate from response codes
                let success =
                    metrics.get(&format!("{}upstream_rq_2xx", prefix)).copied().unwrap_or(0.0);
                let total_rq = metrics
                    .get(&format!("{}upstream_rq_completed", prefix))
                    .copied()
                    .unwrap_or(0.0);
                let success_rate = if total_rq > 0.0 { Some(success / total_rq) } else { None };

                ClusterStats {
                    cluster_name,
                    healthy_hosts,
                    total_hosts,
                    upstream_cx_active,
                    upstream_cx_total,
                    upstream_rq_active,
                    upstream_rq_total,
                    upstream_rq_pending,
                    circuit_breaker_open,
                    outlier_ejections,
                    success_rate,
                }
            })
            .collect()
    }

    /// Extract listener stats from parsed metrics
    fn extract_listener_stats(metrics: &HashMap<String, f64>) -> Vec<ListenerStats> {
        let mut listener_names: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        for name in metrics.keys() {
            // Listener metrics look like: listener.{listener_name}.{metric_name}
            if name.starts_with("listener.") && !name.starts_with("listener_manager.") {
                let parts: Vec<&str> = name.splitn(3, '.').collect();
                if parts.len() >= 2 {
                    listener_names.insert(parts[1].to_string());
                }
            }
        }

        listener_names
            .into_iter()
            .map(|listener_name| {
                let prefix = format!("listener.{}.", listener_name);

                ListenerStats {
                    listener_name,
                    downstream_cx_active: metrics
                        .get(&format!("{}downstream_cx_active", prefix))
                        .copied()
                        .unwrap_or(0.0) as u64,
                    downstream_cx_total: metrics
                        .get(&format!("{}downstream_cx_total", prefix))
                        .copied()
                        .unwrap_or(0.0) as u64,
                    downstream_rq_active: metrics
                        .get(&format!("{}http.ingress_http.downstream_rq_active", prefix))
                        .copied()
                        .unwrap_or(0.0) as u64,
                    downstream_rq_total: metrics
                        .get(&format!("{}http.ingress_http.downstream_rq_total", prefix))
                        .copied()
                        .unwrap_or(0.0) as u64,
                }
            })
            .collect()
    }

    /// Extract server stats from parsed metrics
    fn extract_server_stats(metrics: &HashMap<String, f64>) -> ServerStats {
        ServerStats {
            uptime_seconds: metrics.get("server.uptime").copied().unwrap_or(0.0) as u64,
            memory_allocated: metrics.get("server.memory_allocated").copied().unwrap_or(0.0) as u64,
            memory_heap_size: metrics.get("server.memory_heap_size").copied().unwrap_or(0.0) as u64,
            live_upstreams: metrics.get("cluster_manager.active_clusters").copied().unwrap_or(0.0)
                as u64,
            version: None, // Would need to fetch from /server_info
        }
    }

    /// Extract connection metrics from parsed metrics
    fn extract_connection_metrics(metrics: &HashMap<String, f64>) -> ConnectionMetrics {
        ConnectionMetrics {
            downstream_cx_active: metrics
                .get("http.ingress_http.downstream_cx_active")
                .copied()
                .unwrap_or(0.0) as u64,
            downstream_cx_total: metrics
                .get("http.ingress_http.downstream_cx_total")
                .copied()
                .unwrap_or(0.0) as u64,
            upstream_cx_active: metrics
                .get("cluster_manager.upstream_cx_active")
                .copied()
                .unwrap_or(0.0) as u64,
            upstream_cx_total: metrics
                .get("cluster_manager.upstream_cx_total")
                .copied()
                .unwrap_or(0.0) as u64,
        }
    }

    /// Extract request metrics from parsed metrics
    fn extract_request_metrics(metrics: &HashMap<String, f64>) -> RequestMetrics {
        RequestMetrics {
            active_requests: metrics
                .get("http.ingress_http.downstream_rq_active")
                .copied()
                .unwrap_or(0.0) as u64,
            total_requests: metrics
                .get("http.ingress_http.downstream_rq_total")
                .copied()
                .unwrap_or(0.0) as u64,
            pending_requests: 0, // Aggregated from clusters
            rps: None,           // Computed over time
        }
    }

    /// Extract response code metrics from parsed metrics
    fn extract_response_code_metrics(metrics: &HashMap<String, f64>) -> ResponseCodeMetrics {
        ResponseCodeMetrics {
            xx_2xx: metrics.get("http.ingress_http.downstream_rq_2xx").copied().unwrap_or(0.0)
                as u64,
            xx_3xx: metrics.get("http.ingress_http.downstream_rq_3xx").copied().unwrap_or(0.0)
                as u64,
            xx_4xx: metrics.get("http.ingress_http.downstream_rq_4xx").copied().unwrap_or(0.0)
                as u64,
            xx_5xx: metrics.get("http.ingress_http.downstream_rq_5xx").copied().unwrap_or(0.0)
                as u64,
        }
    }

    /// Extract latency metrics from parsed metrics (histograms)
    fn extract_latency_metrics(metrics: &HashMap<String, f64>) -> LatencyMetrics {
        // Envoy uses histogram metrics for latency
        // Format: http.ingress_http.downstream_rq_time P50/P90/P99
        LatencyMetrics {
            p50_ms: metrics.get("http.ingress_http.downstream_rq_time_P50").copied(),
            p90_ms: metrics.get("http.ingress_http.downstream_rq_time_P90").copied(),
            p99_ms: metrics.get("http.ingress_http.downstream_rq_time_P99").copied(),
            avg_ms: None, // Not directly available from Envoy
        }
    }

    /// Convert parsed metrics to gauge format
    fn to_gauge_metrics(metrics: &HashMap<String, f64>) -> Vec<GaugeMetric> {
        metrics
            .iter()
            .map(|(name, value)| GaugeMetric {
                name: name.clone(),
                value: *value,
                unit: None,
                help_text: None,
            })
            .collect()
    }
}

#[async_trait]
impl StatsDataSource for EnvoyAdminStats {
    async fn fetch_stats(&self, team_id: &str, admin_url: &str) -> Result<StatsSnapshot> {
        let stats_url = format!("{}/stats", admin_url.trim_end_matches('/'));

        debug!("Fetching stats from {} for team {}", stats_url, team_id);

        let response = self.client.get(&stats_url).send().await.map_err(|e| {
            crate::errors::FlowplaneError::internal(format!(
                "Failed to fetch stats from {}: {}",
                stats_url, e
            ))
        })?;

        if !response.status().is_success() {
            return Err(crate::errors::FlowplaneError::internal(format!(
                "Envoy admin API returned status {} for {}",
                response.status(),
                stats_url
            )));
        }

        let text = response.text().await.map_err(|e| {
            crate::errors::FlowplaneError::internal(format!("Failed to read stats response: {}", e))
        })?;

        let metrics = Self::parse_stats_text(&text);

        let clusters = Self::extract_cluster_stats(&metrics);
        let listeners = Self::extract_listener_stats(&metrics);
        let server = Self::extract_server_stats(&metrics);
        let connections = Self::extract_connection_metrics(&metrics);
        let requests = Self::extract_request_metrics(&metrics);
        let response_codes = Self::extract_response_code_metrics(&metrics);
        let latency = Self::extract_latency_metrics(&metrics);
        let gauges = Self::to_gauge_metrics(&metrics);

        let mut snapshot = StatsSnapshot {
            team_id: team_id.to_string(),
            timestamp: chrono::Utc::now(),
            server,
            clusters,
            listeners,
            connections,
            requests,
            response_codes,
            latency,
            health_status: EnvoyHealthStatus::Unknown,
            gauges,
        };

        // Compute overall health from cluster data
        snapshot.compute_health_status();

        Ok(snapshot)
    }

    async fn health_check(&self, admin_url: &str) -> Result<bool> {
        let ready_url = format!("{}/ready", admin_url.trim_end_matches('/'));

        match self.client.get(&ready_url).send().await {
            Ok(response) => Ok(response.status().is_success()),
            Err(e) => {
                warn!("Health check failed for {}: {}", admin_url, e);
                Ok(false)
            }
        }
    }
}

/// Mock implementation for testing
#[cfg(test)]
pub struct MockStatsDataSource {
    pub snapshots: std::sync::Mutex<HashMap<String, StatsSnapshot>>,
    pub health_status: std::sync::Mutex<bool>,
}

#[cfg(test)]
impl MockStatsDataSource {
    pub fn new() -> Self {
        Self {
            snapshots: std::sync::Mutex::new(HashMap::new()),
            health_status: std::sync::Mutex::new(true),
        }
    }

    pub fn set_snapshot(&self, team_id: &str, snapshot: StatsSnapshot) {
        self.snapshots.lock().unwrap().insert(team_id.to_string(), snapshot);
    }

    pub fn set_health(&self, healthy: bool) {
        *self.health_status.lock().unwrap() = healthy;
    }
}

#[cfg(test)]
#[async_trait]
impl StatsDataSource for MockStatsDataSource {
    async fn fetch_stats(&self, team_id: &str, _admin_url: &str) -> Result<StatsSnapshot> {
        self.snapshots
            .lock()
            .unwrap()
            .get(team_id)
            .cloned()
            .ok_or_else(|| crate::errors::FlowplaneError::not_found("StatsSnapshot", team_id))
    }

    async fn health_check(&self, _admin_url: &str) -> Result<bool> {
        Ok(*self.health_status.lock().unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_stats_text_basic() {
        let text = r#"
cluster.api-backend.upstream_cx_active: 10
cluster.api-backend.upstream_cx_total: 500
server.uptime: 86400
"#;

        let metrics = EnvoyAdminStats::parse_stats_text(text);

        assert_eq!(metrics.get("cluster.api-backend.upstream_cx_active"), Some(&10.0));
        assert_eq!(metrics.get("cluster.api-backend.upstream_cx_total"), Some(&500.0));
        assert_eq!(metrics.get("server.uptime"), Some(&86400.0));
    }

    #[test]
    fn test_parse_stats_text_with_type() {
        let text = r#"
cluster.api.upstream_cx_active: 5 (gauge)
cluster.api.upstream_rq_total: 1000 (counter)
"#;

        let metrics = EnvoyAdminStats::parse_stats_text(text);

        assert_eq!(metrics.get("cluster.api.upstream_cx_active"), Some(&5.0));
        assert_eq!(metrics.get("cluster.api.upstream_rq_total"), Some(&1000.0));
    }

    #[test]
    fn test_parse_stats_text_skips_invalid() {
        let text = r#"
cluster.api.upstream_cx_active: 10
cluster.api.histogram: No recorded values
# This is a comment
"#;

        let metrics = EnvoyAdminStats::parse_stats_text(text);

        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics.get("cluster.api.upstream_cx_active"), Some(&10.0));
    }

    #[test]
    fn test_extract_cluster_stats() {
        let mut metrics = HashMap::new();
        metrics.insert("cluster.api-backend.membership_healthy".to_string(), 3.0);
        metrics.insert("cluster.api-backend.membership_total".to_string(), 3.0);
        metrics.insert("cluster.api-backend.upstream_cx_active".to_string(), 10.0);
        metrics.insert("cluster.api-backend.upstream_rq_2xx".to_string(), 900.0);
        metrics.insert("cluster.api-backend.upstream_rq_completed".to_string(), 1000.0);

        let clusters = EnvoyAdminStats::extract_cluster_stats(&metrics);

        assert_eq!(clusters.len(), 1);
        let cluster = &clusters[0];
        assert_eq!(cluster.cluster_name, "api-backend");
        assert_eq!(cluster.healthy_hosts, 3);
        assert_eq!(cluster.total_hosts, 3);
        assert_eq!(cluster.upstream_cx_active, 10);
        assert_eq!(cluster.success_rate, Some(0.9));
    }

    #[test]
    fn test_extract_server_stats() {
        let mut metrics = HashMap::new();
        metrics.insert("server.uptime".to_string(), 86400.0);
        metrics.insert("server.memory_allocated".to_string(), 1000000.0);
        metrics.insert("cluster_manager.active_clusters".to_string(), 5.0);

        let server = EnvoyAdminStats::extract_server_stats(&metrics);

        assert_eq!(server.uptime_seconds, 86400);
        assert_eq!(server.memory_allocated, 1000000);
        assert_eq!(server.live_upstreams, 5);
    }

    #[test]
    fn test_extract_response_codes() {
        let mut metrics = HashMap::new();
        metrics.insert("http.ingress_http.downstream_rq_2xx".to_string(), 9000.0);
        metrics.insert("http.ingress_http.downstream_rq_4xx".to_string(), 800.0);
        metrics.insert("http.ingress_http.downstream_rq_5xx".to_string(), 200.0);

        let codes = EnvoyAdminStats::extract_response_code_metrics(&metrics);

        assert_eq!(codes.xx_2xx, 9000);
        assert_eq!(codes.xx_4xx, 800);
        assert_eq!(codes.xx_5xx, 200);
    }

    #[tokio::test]
    async fn test_mock_data_source() {
        let mock = MockStatsDataSource::new();

        let snapshot = StatsSnapshot::new("test-team".to_string());
        mock.set_snapshot("test-team", snapshot);

        let result = mock.fetch_stats("test-team", "http://localhost:9901").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().team_id, "test-team");
    }

    #[tokio::test]
    async fn test_mock_health_check() {
        let mock = MockStatsDataSource::new();

        assert!(mock.health_check("http://localhost:9901").await.unwrap());

        mock.set_health(false);
        assert!(!mock.health_check("http://localhost:9901").await.unwrap());
    }
}
