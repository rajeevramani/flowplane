//! Stats domain types for Envoy metrics collection
//!
//! This module contains domain types for the Envoy Stats Dashboard feature,
//! including metric representations, stats snapshots, and health status.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A single metric point from any data source
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricPoint {
    /// Metric name (e.g., "cluster.upstream_cx_active")
    pub name: String,
    /// Numeric value
    pub value: f64,
    /// Labels/tags for the metric
    pub labels: HashMap<String, String>,
    /// When this metric was collected
    pub timestamp: DateTime<Utc>,
}

/// A gauge metric (point-in-time value)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GaugeMetric {
    /// Metric name
    pub name: String,
    /// Current value
    pub value: f64,
    /// Unit of measurement (e.g., "connections", "bytes", "ms")
    pub unit: Option<String>,
    /// Human-readable description
    pub help_text: Option<String>,
}

/// Connection-related metrics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConnectionMetrics {
    /// Active downstream connections (clients -> Envoy)
    pub downstream_cx_active: u64,
    /// Total downstream connections
    pub downstream_cx_total: u64,
    /// Active upstream connections (Envoy -> backends)
    pub upstream_cx_active: u64,
    /// Total upstream connections
    pub upstream_cx_total: u64,
}

/// Request-related metrics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RequestMetrics {
    /// Active requests being processed
    pub active_requests: u64,
    /// Total requests processed
    pub total_requests: u64,
    /// Pending requests in queue
    pub pending_requests: u64,
    /// Requests per second (computed)
    pub rps: Option<f64>,
}

/// HTTP response code distribution
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResponseCodeMetrics {
    /// 2xx responses
    pub xx_2xx: u64,
    /// 3xx responses
    pub xx_3xx: u64,
    /// 4xx responses
    pub xx_4xx: u64,
    /// 5xx responses
    pub xx_5xx: u64,
}

/// Latency percentile metrics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LatencyMetrics {
    /// 50th percentile (median) in milliseconds
    pub p50_ms: Option<f64>,
    /// 90th percentile in milliseconds
    pub p90_ms: Option<f64>,
    /// 99th percentile in milliseconds
    pub p99_ms: Option<f64>,
    /// Average latency in milliseconds
    pub avg_ms: Option<f64>,
}

/// Server-level stats from Envoy
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServerStats {
    /// Envoy uptime in seconds
    pub uptime_seconds: u64,
    /// Total allocated memory in bytes
    pub memory_allocated: u64,
    /// Total heap size in bytes
    pub memory_heap_size: u64,
    /// Number of live (healthy) upstreams
    pub live_upstreams: u64,
    /// Envoy version string
    pub version: Option<String>,
}

/// Per-cluster stats
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClusterStats {
    /// Cluster name
    pub cluster_name: String,
    /// Number of healthy hosts
    pub healthy_hosts: u64,
    /// Total number of hosts
    pub total_hosts: u64,
    /// Active connections to this cluster
    pub upstream_cx_active: u64,
    /// Total connections to this cluster
    pub upstream_cx_total: u64,
    /// Active requests to this cluster
    pub upstream_rq_active: u64,
    /// Total requests to this cluster
    pub upstream_rq_total: u64,
    /// Pending requests in queue
    pub upstream_rq_pending: u64,
    /// Circuit breaker open/closed
    pub circuit_breaker_open: bool,
    /// Number of outlier ejections
    pub outlier_ejections: u64,
    /// Success rate (0.0 - 1.0)
    pub success_rate: Option<f64>,
}

/// Per-listener stats
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListenerStats {
    /// Listener name
    pub listener_name: String,
    /// Active downstream connections
    pub downstream_cx_active: u64,
    /// Total downstream connections
    pub downstream_cx_total: u64,
    /// Active requests on this listener
    pub downstream_rq_active: u64,
    /// Total requests on this listener
    pub downstream_rq_total: u64,
}

/// Health status of an Envoy instance or cluster
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EnvoyHealthStatus {
    /// All hosts healthy, no issues
    Healthy,
    /// Some hosts unhealthy or degraded performance
    Degraded,
    /// Majority hosts unhealthy or critical issues
    Unhealthy,
    /// Status unknown (e.g., cannot reach admin port)
    #[default]
    Unknown,
}

impl EnvoyHealthStatus {
    /// Compute health status from healthy/total host counts
    pub fn from_host_counts(healthy: u64, total: u64) -> Self {
        if total == 0 {
            return Self::Unknown;
        }
        let ratio = healthy as f64 / total as f64;
        if ratio >= 1.0 {
            Self::Healthy
        } else if ratio >= 0.5 {
            Self::Degraded
        } else {
            Self::Unhealthy
        }
    }
}

/// Complete stats snapshot from an Envoy instance
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StatsSnapshot {
    /// Team this snapshot belongs to
    pub team_id: String,
    /// When this snapshot was collected
    pub timestamp: DateTime<Utc>,
    /// Server-level stats
    pub server: ServerStats,
    /// Per-cluster stats
    pub clusters: Vec<ClusterStats>,
    /// Per-listener stats
    pub listeners: Vec<ListenerStats>,
    /// Aggregated connection metrics
    pub connections: ConnectionMetrics,
    /// Aggregated request metrics
    pub requests: RequestMetrics,
    /// Response code distribution
    pub response_codes: ResponseCodeMetrics,
    /// Latency percentiles
    pub latency: LatencyMetrics,
    /// Overall health status
    pub health_status: EnvoyHealthStatus,
    /// Raw gauge metrics (for detailed view)
    pub gauges: Vec<GaugeMetric>,
}

impl StatsSnapshot {
    /// Create a new empty snapshot for a team
    pub fn new(team_id: String) -> Self {
        Self { team_id, timestamp: Utc::now(), ..Default::default() }
    }

    /// Compute overall health status from cluster stats
    pub fn compute_health_status(&mut self) {
        if self.clusters.is_empty() {
            self.health_status = EnvoyHealthStatus::Unknown;
            return;
        }

        let total_healthy: u64 = self.clusters.iter().map(|c| c.healthy_hosts).sum();
        let total_hosts: u64 = self.clusters.iter().map(|c| c.total_hosts).sum();
        let any_cb_open = self.clusters.iter().any(|c| c.circuit_breaker_open);

        if any_cb_open {
            self.health_status = EnvoyHealthStatus::Unhealthy;
        } else {
            self.health_status = EnvoyHealthStatus::from_host_counts(total_healthy, total_hosts);
        }
    }
}

/// Filters for querying stats
#[derive(Debug, Clone, Default, Deserialize)]
pub struct StatsFilters {
    /// Only include metrics matching these patterns (glob-style)
    pub include_patterns: Vec<String>,
    /// Exclude metrics matching these patterns
    pub exclude_patterns: Vec<String>,
    /// Only include specific clusters
    pub cluster_names: Vec<String>,
    /// Only include specific listeners
    pub listener_names: Vec<String>,
}

/// Overview stats for the dashboard home page
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StatsOverview {
    /// Total requests per second across all clusters
    pub total_rps: f64,
    /// Total active connections
    pub total_connections: u64,
    /// Overall error rate (0.0 - 1.0)
    pub error_rate: f64,
    /// P99 latency in milliseconds
    pub p99_latency_ms: f64,
    /// Number of healthy clusters
    pub healthy_clusters: u64,
    /// Number of degraded clusters
    pub degraded_clusters: u64,
    /// Number of unhealthy clusters
    pub unhealthy_clusters: u64,
    /// Total clusters
    pub total_clusters: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_status_from_host_counts() {
        assert_eq!(EnvoyHealthStatus::from_host_counts(3, 3), EnvoyHealthStatus::Healthy);
        assert_eq!(EnvoyHealthStatus::from_host_counts(2, 3), EnvoyHealthStatus::Degraded);
        assert_eq!(EnvoyHealthStatus::from_host_counts(1, 3), EnvoyHealthStatus::Unhealthy);
        assert_eq!(EnvoyHealthStatus::from_host_counts(0, 0), EnvoyHealthStatus::Unknown);
    }

    #[test]
    fn test_stats_snapshot_compute_health() {
        let mut snapshot = StatsSnapshot::new("test-team".to_string());
        snapshot.clusters = vec![
            ClusterStats {
                cluster_name: "cluster-1".to_string(),
                healthy_hosts: 3,
                total_hosts: 3,
                ..Default::default()
            },
            ClusterStats {
                cluster_name: "cluster-2".to_string(),
                healthy_hosts: 2,
                total_hosts: 3,
                ..Default::default()
            },
        ];

        snapshot.compute_health_status();
        // 5/6 healthy = 83% = Degraded
        assert_eq!(snapshot.health_status, EnvoyHealthStatus::Degraded);
    }

    #[test]
    fn test_stats_snapshot_unhealthy_when_cb_open() {
        let mut snapshot = StatsSnapshot::new("test-team".to_string());
        snapshot.clusters = vec![ClusterStats {
            cluster_name: "cluster-1".to_string(),
            healthy_hosts: 3,
            total_hosts: 3,
            circuit_breaker_open: true,
            ..Default::default()
        }];

        snapshot.compute_health_status();
        assert_eq!(snapshot.health_status, EnvoyHealthStatus::Unhealthy);
    }
}
