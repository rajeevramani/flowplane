//! Dataplane API types and DTOs

use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use validator::Validate;

/// Request body for creating a new dataplane
#[derive(Debug, Clone, Serialize, Deserialize, Validate, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateDataplaneBody {
    /// Team name for the dataplane
    #[validate(length(min = 1, max = 100))]
    pub team: String,

    /// Unique name for the dataplane within the team
    #[validate(length(min = 1, max = 100))]
    pub name: String,

    /// Gateway host address for MCP tool execution (e.g., "10.0.0.5" or "envoy.example.com")
    #[validate(length(max = 255))]
    pub gateway_host: Option<String>,

    /// Optional description for the dataplane
    #[validate(length(max = 500))]
    pub description: Option<String>,
}

/// Request body for updating an existing dataplane
#[derive(Debug, Clone, Serialize, Deserialize, Validate, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateDataplaneBody {
    /// Gateway host address for MCP tool execution
    #[validate(length(max = 255))]
    pub gateway_host: Option<String>,

    /// Optional description for the dataplane
    #[validate(length(max = 500))]
    pub description: Option<String>,
}

/// Response for a single dataplane
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DataplaneResponse {
    /// Unique identifier for the dataplane
    pub id: String,

    /// Team name
    pub team: String,

    /// Dataplane name
    pub name: String,

    /// Gateway host address
    pub gateway_host: Option<String>,

    /// Description
    pub description: Option<String>,

    /// Creation timestamp
    pub created_at: chrono::DateTime<chrono::Utc>,

    /// Last update timestamp
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Response for listing dataplanes
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListDataplanesResponse {
    pub dataplanes: Vec<DataplaneResponse>,
}

/// Query parameters for listing dataplanes
#[derive(Debug, Clone, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct ListDataplanesQuery {
    /// Maximum number of results
    pub limit: Option<i32>,

    /// Offset for pagination
    pub offset: Option<i32>,
}

/// Path parameter for team-scoped dataplane operations
#[derive(Debug, Clone, Deserialize, IntoParams)]
pub struct TeamDataplanePath {
    /// Team name
    pub team: String,

    /// Dataplane name
    pub name: String,
}

/// Query parameters for dataplane bootstrap endpoint
#[derive(Debug, Clone, Deserialize, Serialize, IntoParams, ToSchema)]
pub struct BootstrapQuery {
    /// Output format: yaml or json (default: yaml)
    #[serde(default)]
    #[param(required = false)]
    pub format: Option<String>,

    /// Enable mTLS configuration in bootstrap. When true, adds transport_socket
    /// with TLS settings to the xds_cluster. Defaults to true if control plane
    /// has mTLS configured.
    #[serde(default)]
    #[param(required = false)]
    pub mtls: Option<bool>,

    /// Path to client certificate file (default: /etc/envoy/certs/client.pem)
    #[serde(default)]
    #[param(required = false)]
    pub cert_path: Option<String>,

    /// Path to client private key file (default: /etc/envoy/certs/client-key.pem)
    #[serde(default)]
    #[param(required = false)]
    pub key_path: Option<String>,

    /// Path to CA certificate file (default: /etc/envoy/certs/ca.pem)
    #[serde(default)]
    #[param(required = false)]
    pub ca_path: Option<String>,
}

impl From<crate::storage::repositories::DataplaneData> for DataplaneResponse {
    fn from(data: crate::storage::repositories::DataplaneData) -> Self {
        Self {
            id: data.id.to_string(),
            team: data.team,
            name: data.name,
            gateway_host: data.gateway_host,
            description: data.description,
            created_at: data.created_at,
            updated_at: data.updated_at,
        }
    }
}
