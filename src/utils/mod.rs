//! Utility functions and helpers

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// Regex for validating Envoy resource names
/// Names must start with a letter or underscore, followed by letters, numbers, underscores, or hyphens
pub static VALID_NAME_REGEX: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(r"^[a-zA-Z_][a-zA-Z0-9_-]*$").unwrap());

/// Generate a new UUID v4 as a string
pub fn generate_id() -> String {
    Uuid::new_v4().to_string()
}

/// Configuration version for atomic updates
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConfigVersion {
    pub version: u64,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl ConfigVersion {
    /// Create a new configuration version
    pub fn new(version: u64) -> Self {
        Self { version, timestamp: chrono::Utc::now() }
    }

    /// Get the next version number
    pub fn next(&self) -> Self {
        Self::new(self.version + 1)
    }
}

/// Standard response wrapper for API endpoints
#[derive(Debug, Serialize, Deserialize)]
pub struct ApiResponse<T> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<String>,
    pub version: Option<ConfigVersion>,
}

impl<T> ApiResponse<T> {
    /// Create a successful response
    pub fn success(data: T) -> Self {
        Self { success: true, data: Some(data), error: None, version: None }
    }

    /// Create a successful response with version
    pub fn success_with_version(data: T, version: ConfigVersion) -> Self {
        Self { success: true, data: Some(data), error: None, version: Some(version) }
    }

    /// Create an error response
    pub fn error(message: String) -> Self {
        Self { success: false, data: None, error: Some(message), version: None }
    }
}

/// Common pagination parameters
#[derive(Debug, Deserialize)]
pub struct PaginationParams {
    #[serde(default = "default_page")]
    pub page: u32,
    #[serde(default = "default_limit")]
    pub limit: u32,
}

fn default_page() -> u32 {
    1
}

fn default_limit() -> u32 {
    50
}

impl PaginationParams {
    /// Calculate offset for database queries
    pub fn offset(&self) -> u32 {
        (self.page - 1) * self.limit
    }

    /// Validate pagination parameters
    pub fn validate(&self) -> crate::Result<()> {
        if self.page == 0 {
            return Err(crate::Error::Validation("Page must be >= 1".to_string()));
        }
        if self.limit == 0 || self.limit > 1000 {
            return Err(crate::Error::Validation("Limit must be 1-1000".to_string()));
        }
        Ok(())
    }
}

/// Paginated response wrapper
#[derive(Debug, Serialize)]
pub struct PaginatedResponse<T> {
    pub items: Vec<T>,
    pub total: u64,
    pub page: u32,
    pub limit: u32,
    pub total_pages: u32,
}

impl<T> PaginatedResponse<T> {
    /// Create a new paginated response
    pub fn new(items: Vec<T>, total: u64, page: u32, limit: u32) -> Self {
        let total_pages = ((total as f64) / (limit as f64)).ceil() as u32;
        Self { items, total, page, limit, total_pages }
    }
}

/// Correlation ID for request tracing
#[derive(Debug, Clone)]
pub struct CorrelationId(String);

impl Default for CorrelationId {
    fn default() -> Self {
        Self::new()
    }
}

impl CorrelationId {
    /// Generate a new correlation ID
    pub fn new() -> Self {
        Self(generate_id())
    }

    /// Get the correlation ID as a string
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for CorrelationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Health check status
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
}

/// Health check result
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HealthCheck {
    pub status: HealthStatus,
    pub component: String,
    pub details: HashMap<String, String>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl HealthCheck {
    /// Create a healthy status
    pub fn healthy(component: &str) -> Self {
        Self {
            status: HealthStatus::Healthy,
            component: component.to_string(),
            details: HashMap::new(),
            timestamp: chrono::Utc::now(),
        }
    }

    /// Create a degraded status with details
    pub fn degraded(component: &str, details: HashMap<String, String>) -> Self {
        Self {
            status: HealthStatus::Degraded,
            component: component.to_string(),
            details,
            timestamp: chrono::Utc::now(),
        }
    }

    /// Create an unhealthy status with details
    pub fn unhealthy(component: &str, details: HashMap<String, String>) -> Self {
        Self {
            status: HealthStatus::Unhealthy,
            component: component.to_string(),
            details,
            timestamp: chrono::Utc::now(),
        }
    }
}
