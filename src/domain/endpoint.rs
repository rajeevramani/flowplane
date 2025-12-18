//! Cluster Endpoint Domain Types
//!
//! This module contains domain types for cluster endpoints that are
//! extracted from cluster configuration JSON into normalized tables.

use serde::{Deserialize, Serialize};
use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use sqlx::{Decode, Encode, Sqlite, Type};
use std::fmt;
use std::str::FromStr;
use utoipa::ToSchema;

/// Health status of a cluster endpoint.
///
/// Tracks the current health state of an endpoint as determined by
/// active health checks or passive outlier detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum EndpointHealthStatus {
    /// Endpoint is healthy and accepting traffic
    Healthy,
    /// Endpoint is unhealthy and should not receive traffic
    Unhealthy,
    /// Endpoint is degraded (partially healthy)
    Degraded,
    /// Health status is unknown (not yet determined)
    #[default]
    Unknown,
}

impl EndpointHealthStatus {
    /// Convert to database string representation
    pub fn as_str(&self) -> &'static str {
        match self {
            EndpointHealthStatus::Healthy => "healthy",
            EndpointHealthStatus::Unhealthy => "unhealthy",
            EndpointHealthStatus::Degraded => "degraded",
            EndpointHealthStatus::Unknown => "unknown",
        }
    }

    /// Check if the endpoint should receive traffic
    pub fn is_available(&self) -> bool {
        matches!(self, EndpointHealthStatus::Healthy | EndpointHealthStatus::Degraded)
    }
}

impl fmt::Display for EndpointHealthStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for EndpointHealthStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "healthy" => Ok(EndpointHealthStatus::Healthy),
            "unhealthy" => Ok(EndpointHealthStatus::Unhealthy),
            "degraded" => Ok(EndpointHealthStatus::Degraded),
            "unknown" => Ok(EndpointHealthStatus::Unknown),
            _ => Err(format!("Invalid health status: {}", s)),
        }
    }
}

// SQLx trait implementations for database compatibility
impl Type<Sqlite> for EndpointHealthStatus {
    fn type_info() -> sqlx::sqlite::SqliteTypeInfo {
        <String as Type<Sqlite>>::type_info()
    }
}

impl<'q> Encode<'q, Sqlite> for EndpointHealthStatus {
    fn encode_by_ref(
        &self,
        buf: &mut Vec<sqlx::sqlite::SqliteArgumentValue<'q>>,
    ) -> Result<IsNull, BoxDynError> {
        <&str as Encode<'q, Sqlite>>::encode_by_ref(&self.as_str(), buf)
    }
}

impl<'r> Decode<'r, Sqlite> for EndpointHealthStatus {
    fn decode(value: sqlx::sqlite::SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        let s = <String as Decode<'r, Sqlite>>::decode(value)?;
        EndpointHealthStatus::from_str(&s).map_err(|e| e.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_status_string_roundtrip() {
        for status in [
            EndpointHealthStatus::Healthy,
            EndpointHealthStatus::Unhealthy,
            EndpointHealthStatus::Degraded,
            EndpointHealthStatus::Unknown,
        ] {
            let s = status.as_str();
            let parsed: EndpointHealthStatus = s.parse().expect("Failed to parse");
            assert_eq!(status, parsed);
        }
    }

    #[test]
    fn health_status_availability() {
        assert!(EndpointHealthStatus::Healthy.is_available());
        assert!(EndpointHealthStatus::Degraded.is_available());
        assert!(!EndpointHealthStatus::Unhealthy.is_available());
        assert!(!EndpointHealthStatus::Unknown.is_available());
    }

    #[test]
    fn health_status_default() {
        let status: EndpointHealthStatus = Default::default();
        assert_eq!(status, EndpointHealthStatus::Unknown);
    }

    #[test]
    fn health_status_serialization() {
        let status = EndpointHealthStatus::Healthy;
        let json = serde_json::to_string(&status).expect("Failed to serialize");
        assert_eq!(json, "\"healthy\"");

        let deserialized: EndpointHealthStatus =
            serde_json::from_str(&json).expect("Failed to deserialize");
        assert_eq!(status, deserialized);
    }
}
