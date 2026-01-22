//! Envoy stats parsing and verification utilities
//!
//! Provides helpers for parsing Envoy stats and verifying expected values
//! in E2E tests.

use std::collections::HashMap;

/// Parsed Envoy stats
#[derive(Debug, Default)]
pub struct EnvoyStats {
    /// Raw stats as key-value pairs
    pub stats: HashMap<String, i64>,
}

impl EnvoyStats {
    /// Parse stats from Envoy's text format
    ///
    /// Format is: `stat_name: value\n`
    pub fn parse(raw: &str) -> Self {
        let stats = raw
            .lines()
            .filter_map(|line| {
                let parts: Vec<&str> = line.splitn(2, ": ").collect();
                if parts.len() == 2 {
                    let name = parts[0].trim().to_string();
                    let value = parts[1].trim().parse::<i64>().ok()?;
                    Some((name, value))
                } else {
                    None
                }
            })
            .collect();

        Self { stats }
    }

    /// Parse stats from Envoy's JSON format
    pub fn parse_json(json: &serde_json::Value) -> Self {
        let mut stats = HashMap::new();

        if let Some(stats_array) = json["stats"].as_array() {
            for stat in stats_array {
                if let (Some(name), Some(value)) = (stat["name"].as_str(), stat["value"].as_i64()) {
                    stats.insert(name.to_string(), value);
                }
            }
        }

        Self { stats }
    }

    /// Get a specific stat value
    pub fn get(&self, name: &str) -> Option<i64> {
        self.stats.get(name).copied()
    }

    /// Get stat value with default
    pub fn get_or(&self, name: &str, default: i64) -> i64 {
        self.stats.get(name).copied().unwrap_or(default)
    }

    /// Find stats matching a pattern (simple contains match)
    pub fn find_matching(&self, pattern: &str) -> HashMap<String, i64> {
        self.stats
            .iter()
            .filter(|(k, _)| k.contains(pattern))
            .map(|(k, v)| (k.clone(), *v))
            .collect()
    }

    /// Get upstream request count for a cluster
    pub fn upstream_rq_total(&self, cluster: &str) -> i64 {
        self.get_or(&format!("cluster.{}.upstream_rq_total", cluster), 0)
    }

    /// Get upstream retry count for a cluster
    pub fn upstream_rq_retry(&self, cluster: &str) -> i64 {
        self.get_or(&format!("cluster.{}.upstream_rq_retry", cluster), 0)
    }

    /// Get upstream connection overflow count for a cluster (circuit breaker triggered)
    pub fn upstream_cx_overflow(&self, cluster: &str) -> i64 {
        self.get_or(&format!("cluster.{}.upstream_cx_overflow", cluster), 0)
    }

    /// Get rate limit stats for a filter
    pub fn rate_limit_over_limit(&self, stat_prefix: &str) -> i64 {
        self.get_or(&format!("http.{}.ratelimit.over_limit", stat_prefix), 0)
    }

    /// Get compressor stats
    pub fn compressor_compressed(&self, stat_prefix: &str, compressor: &str) -> i64 {
        self.get_or(&format!("http.{}.compressor.{}.compressed", stat_prefix, compressor), 0)
    }

    /// Get ext_authz stats
    pub fn ext_authz_ok(&self, stat_prefix: &str) -> i64 {
        self.get_or(&format!("http.{}.ext_authz.ok", stat_prefix), 0)
    }

    /// Get ext_authz denied stats
    pub fn ext_authz_denied(&self, stat_prefix: &str) -> i64 {
        self.get_or(&format!("http.{}.ext_authz.denied", stat_prefix), 0)
    }

    /// Get JWT auth filter stats
    pub fn jwt_auth_allowed(&self, stat_prefix: &str) -> i64 {
        self.get_or(&format!("http.{}.jwt_authn.allowed", stat_prefix), 0)
    }

    /// Get JWT auth filter denied stats
    pub fn jwt_auth_denied(&self, stat_prefix: &str) -> i64 {
        self.get_or(&format!("http.{}.jwt_authn.denied", stat_prefix), 0)
    }

    /// Get outlier detection ejection stats
    pub fn outlier_ejections_total(&self, cluster: &str) -> i64 {
        self.get_or(&format!("cluster.{}.outlier_detection.ejections_total", cluster), 0)
    }

    /// Assert a stat equals expected value
    pub fn assert_stat(&self, name: &str, expected: i64) {
        let actual = self.get(name);
        assert_eq!(
            actual,
            Some(expected),
            "Stat '{}' expected {} but got {:?}",
            name,
            expected,
            actual
        );
    }

    /// Assert a stat is at least the expected value
    pub fn assert_stat_gte(&self, name: &str, min: i64) {
        let actual = self.get_or(name, 0);
        assert!(actual >= min, "Stat '{}' expected >= {} but got {}", name, min, actual);
    }
}

/// Stat assertions for cleaner test code
pub trait StatAssertions {
    /// Assert retry count for cluster
    fn assert_retries(&self, cluster: &str, min_count: i64);
    /// Assert circuit breaker triggered
    fn assert_circuit_breaker_triggered(&self, cluster: &str);
    /// Assert rate limit exceeded
    fn assert_rate_limited(&self, stat_prefix: &str, min_count: i64);
}

impl StatAssertions for EnvoyStats {
    fn assert_retries(&self, cluster: &str, min_count: i64) {
        let retries = self.upstream_rq_retry(cluster);
        assert!(
            retries >= min_count,
            "Expected at least {} retries for cluster '{}', got {}",
            min_count,
            cluster,
            retries
        );
    }

    fn assert_circuit_breaker_triggered(&self, cluster: &str) {
        let overflow = self.upstream_cx_overflow(cluster);
        assert!(
            overflow > 0,
            "Expected circuit breaker overflow for cluster '{}', got {}",
            cluster,
            overflow
        );
    }

    fn assert_rate_limited(&self, stat_prefix: &str, min_count: i64) {
        let over_limit = self.rate_limit_over_limit(stat_prefix);
        assert!(
            over_limit >= min_count,
            "Expected at least {} rate limit violations for '{}', got {}",
            min_count,
            stat_prefix,
            over_limit
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_text_stats() {
        let raw = r#"
cluster.test-cluster.upstream_rq_total: 100
cluster.test-cluster.upstream_rq_retry: 5
http.ingress.ratelimit.over_limit: 10
"#;

        let stats = EnvoyStats::parse(raw);
        assert_eq!(stats.get("cluster.test-cluster.upstream_rq_total"), Some(100));
        assert_eq!(stats.get("cluster.test-cluster.upstream_rq_retry"), Some(5));
        assert_eq!(stats.upstream_rq_total("test-cluster"), 100);
        assert_eq!(stats.upstream_rq_retry("test-cluster"), 5);
    }

    #[test]
    fn test_find_matching() {
        let raw = r#"
cluster.foo.upstream_rq_total: 10
cluster.foo.upstream_rq_retry: 2
cluster.bar.upstream_rq_total: 20
"#;

        let stats = EnvoyStats::parse(raw);
        let foo_stats = stats.find_matching("cluster.foo");
        assert_eq!(foo_stats.len(), 2);
        assert_eq!(foo_stats.get("cluster.foo.upstream_rq_total"), Some(&10));
    }

    #[test]
    fn test_stat_assertions() {
        let raw = r#"
cluster.test.upstream_rq_retry: 5
cluster.test.upstream_cx_overflow: 3
"#;

        let stats = EnvoyStats::parse(raw);
        stats.assert_retries("test", 3);
        stats.assert_circuit_breaker_triggered("test");
    }
}
