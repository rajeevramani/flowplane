//! # Health Checking
//!
//! Provides health checking capabilities for the control plane components.

use crate::errors::{FlowplaneError, Result};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

/// Health status for a component
#[derive(Debug, Clone, PartialEq)]
pub enum HealthStatus {
    /// Component is healthy and operational
    Healthy,
    /// Component is degraded but still functional
    Degraded { message: String },
    /// Component is unhealthy and not functional
    Unhealthy { message: String },
}

impl HealthStatus {
    /// Check if the status is healthy
    pub fn is_healthy(&self) -> bool {
        matches!(self, HealthStatus::Healthy)
    }

    /// Check if the status is operational (healthy or degraded)
    pub fn is_operational(&self) -> bool {
        matches!(self, HealthStatus::Healthy | HealthStatus::Degraded { .. })
    }

    /// Get the status message
    pub fn message(&self) -> Option<&str> {
        match self {
            HealthStatus::Healthy => None,
            HealthStatus::Degraded { message } | HealthStatus::Unhealthy { message } => Some(message),
        }
    }
}

/// Health check result for a component
#[derive(Debug, Clone)]
pub struct HealthCheck {
    /// Component name
    pub component: String,
    /// Health status
    pub status: HealthStatus,
    /// Last check timestamp
    pub last_check: chrono::DateTime<chrono::Utc>,
    /// Additional metadata
    pub metadata: HashMap<String, String>,
}

impl HealthCheck {
    /// Create a new health check result
    pub fn new(component: String, status: HealthStatus) -> Self {
        Self {
            component,
            status,
            last_check: chrono::Utc::now(),
            metadata: HashMap::new(),
        }
    }

    /// Create a healthy health check
    pub fn healthy(component: String) -> Self {
        Self::new(component, HealthStatus::Healthy)
    }

    /// Create a degraded health check
    pub fn degraded<S: Into<String>>(component: String, message: S) -> Self {
        Self::new(component, HealthStatus::Degraded {
            message: message.into(),
        })
    }

    /// Create an unhealthy health check
    pub fn unhealthy<S: Into<String>>(component: String, message: S) -> Self {
        Self::new(component, HealthStatus::Unhealthy {
            message: message.into(),
        })
    }

    /// Add metadata to the health check
    pub fn with_metadata<K: Into<String>, V: Into<String>>(mut self, key: K, value: V) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }
}

/// Component that provides health checking functionality
pub trait HealthProvider {
    /// Perform a health check for this component
    async fn health_check(&self) -> Result<HealthCheck>;
}

/// Central health checker that manages health checks for all components
#[derive(Debug, Clone)]
pub struct HealthChecker {
    /// Registered health providers
    providers: Arc<RwLock<HashMap<String, Box<dyn HealthProvider + Send + Sync>>>>,
    /// Cached health check results
    cache: Arc<RwLock<HashMap<String, HealthCheck>>>,
    /// Unique instance ID
    instance_id: String,
}

impl HealthChecker {
    /// Create a new health checker
    pub fn new() -> Self {
        Self {
            providers: Arc::new(RwLock::new(HashMap::new())),
            cache: Arc::new(RwLock::new(HashMap::new())),
            instance_id: Uuid::new_v4().to_string(),
        }
    }

    /// Register a health provider
    pub async fn register_provider<S: Into<String>>(
        &self,
        name: S,
        provider: Box<dyn HealthProvider + Send + Sync>,
    ) {
        let name = name.into();
        self.providers.write().await.insert(name, provider);
    }

    /// Perform health checks for all registered providers
    pub async fn check_all(&self) -> Result<HashMap<String, HealthCheck>> {
        let providers = self.providers.read().await;
        let mut results = HashMap::new();

        for (name, provider) in providers.iter() {
            let check = match provider.health_check().await {
                Ok(check) => check,
                Err(e) => HealthCheck::unhealthy(
                    name.clone(),
                    format!("Health check failed: {}", e),
                ),
            };
            results.insert(name.clone(), check.clone());
        }

        // Update cache
        let mut cache = self.cache.write().await;
        cache.extend(results.clone());

        Ok(results)
    }

    /// Get the overall health status
    pub async fn overall_status(&self) -> HealthStatus {
        let checks = match self.check_all().await {
            Ok(checks) => checks,
            Err(e) => {
                return HealthStatus::Unhealthy {
                    message: format!("Failed to perform health checks: {}", e),
                };
            }
        };

        if checks.is_empty() {
            return HealthStatus::Degraded {
                message: "No health providers registered".to_string(),
            };
        }

        let mut unhealthy_count = 0;
        let mut degraded_count = 0;

        for check in checks.values() {
            match &check.status {
                HealthStatus::Healthy => {}
                HealthStatus::Degraded { .. } => degraded_count += 1,
                HealthStatus::Unhealthy { .. } => unhealthy_count += 1,
            }
        }

        if unhealthy_count > 0 {
            HealthStatus::Unhealthy {
                message: format!(
                    "{} unhealthy, {} degraded out of {} components",
                    unhealthy_count,
                    degraded_count,
                    checks.len()
                ),
            }
        } else if degraded_count > 0 {
            HealthStatus::Degraded {
                message: format!(
                    "{} degraded out of {} components",
                    degraded_count,
                    checks.len()
                ),
            }
        } else {
            HealthStatus::Healthy
        }
    }

    /// Check if the system is ready to serve traffic
    pub async fn is_ready(&self) -> bool {
        self.overall_status().await.is_operational()
    }

    /// Check if the system is alive (basic liveness check)
    pub async fn is_alive(&self) -> bool {
        // Basic liveness check - we're alive if we can respond
        true
    }

    /// Get the instance ID
    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    /// Get cached health check results
    pub async fn get_cached_checks(&self) -> HashMap<String, HealthCheck> {
        self.cache.read().await.clone()
    }
}

impl Default for HealthChecker {
    fn default() -> Self {
        Self::new()
    }
}

/// Database health provider
pub struct DatabaseHealthProvider {
    db_pool: crate::storage::DbPool,
}

impl DatabaseHealthProvider {
    /// Create a new database health provider
    pub fn new(db_pool: crate::storage::DbPool) -> Self {
        Self { db_pool }
    }
}

#[async_trait::async_trait]
impl HealthProvider for DatabaseHealthProvider {
    async fn health_check(&self) -> Result<HealthCheck> {
        let start = std::time::Instant::now();

        match sqlx::query("SELECT 1").fetch_one(&self.db_pool).await {
            Ok(_) => {
                let duration = start.elapsed();
                Ok(HealthCheck::healthy("database".to_string())
                    .with_metadata("response_time_ms", duration.as_millis().to_string())
                    .with_metadata("active_connections", self.db_pool.size().to_string()))
            }
            Err(e) => Ok(HealthCheck::unhealthy(
                "database".to_string(),
                format!("Database connection failed: {}", e),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_status() {
        assert!(HealthStatus::Healthy.is_healthy());
        assert!(HealthStatus::Healthy.is_operational());
        assert!(HealthStatus::Healthy.message().is_none());

        let degraded = HealthStatus::Degraded {
            message: "slow".to_string(),
        };
        assert!(!degraded.is_healthy());
        assert!(degraded.is_operational());
        assert_eq!(degraded.message(), Some("slow"));

        let unhealthy = HealthStatus::Unhealthy {
            message: "down".to_string(),
        };
        assert!(!unhealthy.is_healthy());
        assert!(!unhealthy.is_operational());
        assert_eq!(unhealthy.message(), Some("down"));
    }

    #[test]
    fn test_health_check_creation() {
        let check = HealthCheck::healthy("test".to_string());
        assert_eq!(check.component, "test");
        assert!(check.status.is_healthy());

        let check = HealthCheck::degraded("test".to_string(), "slow response");
        assert!(!check.status.is_healthy());
        assert!(check.status.is_operational());

        let check = HealthCheck::unhealthy("test".to_string(), "connection failed");
        assert!(!check.status.is_healthy());
        assert!(!check.status.is_operational());
    }

    #[test]
    fn test_health_check_metadata() {
        let check = HealthCheck::healthy("test".to_string())
            .with_metadata("version", "1.0.0")
            .with_metadata("uptime", "3600");

        assert_eq!(check.metadata.get("version"), Some(&"1.0.0".to_string()));
        assert_eq!(check.metadata.get("uptime"), Some(&"3600".to_string()));
    }

    #[tokio::test]
    async fn test_health_checker() {
        let health_checker = HealthChecker::new();

        // Test with no providers
        let status = health_checker.overall_status().await;
        assert!(matches!(status, HealthStatus::Degraded { .. }));

        // Test basic functionality
        assert!(health_checker.is_alive().await);
        assert!(!health_checker.is_ready().await); // Not ready with no healthy providers

        let checks = health_checker.get_cached_checks().await;
        assert!(checks.is_empty());
    }

    // Mock health provider for testing
    struct MockHealthProvider {
        status: HealthStatus,
    }

    #[async_trait::async_trait]
    impl HealthProvider for MockHealthProvider {
        async fn health_check(&self) -> Result<HealthCheck> {
            Ok(HealthCheck::new("mock".to_string(), self.status.clone()))
        }
    }

    #[tokio::test]
    async fn test_health_checker_with_providers() {
        let health_checker = HealthChecker::new();

        // Register a healthy provider
        let healthy_provider = Box::new(MockHealthProvider {
            status: HealthStatus::Healthy,
        });
        health_checker
            .register_provider("service1", healthy_provider)
            .await;

        // Register a degraded provider
        let degraded_provider = Box::new(MockHealthProvider {
            status: HealthStatus::Degraded {
                message: "slow".to_string(),
            },
        });
        health_checker
            .register_provider("service2", degraded_provider)
            .await;

        let status = health_checker.overall_status().await;
        assert!(matches!(status, HealthStatus::Degraded { .. }));
        assert!(health_checker.is_ready().await);

        // Add an unhealthy provider
        let unhealthy_provider = Box::new(MockHealthProvider {
            status: HealthStatus::Unhealthy {
                message: "down".to_string(),
            },
        });
        health_checker
            .register_provider("service3", unhealthy_provider)
            .await;

        let status = health_checker.overall_status().await;
        assert!(matches!(status, HealthStatus::Unhealthy { .. }));
        assert!(!health_checker.is_ready().await);
    }
}