//! Request and response types for filter API handlers

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::domain::{AttachmentPoint, FilterConfig, FilterType};
use crate::storage::FilterData;
use crate::xds::ClusterSpec;

/// Mode for cluster handling when creating filters
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClusterMode {
    /// Create a new cluster with the provided configuration
    Create,
    /// Reuse an existing cluster by name
    #[default]
    Reuse,
}

/// Configuration for cluster creation or reuse when creating a filter
///
/// Filters like OAuth2, JWT Auth, and ExtAuthz require clusters for their
/// backend services. This config allows creating a new cluster inline or
/// reusing an existing one.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClusterCreationConfig {
    /// Mode: "create" to create a new cluster, "reuse" to use existing
    #[serde(default)]
    pub mode: ClusterMode,
    /// Name of the cluster to create or reuse
    pub cluster_name: String,
    /// Service name for the cluster (required when mode is "create")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_name: Option<String>,
    /// Cluster specification (required when mode is "create")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cluster_spec: Option<ClusterSpec>,
    /// Team for the cluster (defaults to filter's team if not specified)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub team: Option<String>,
}

impl ClusterCreationConfig {
    /// Validate the cluster creation config
    pub fn validate(&self) -> Result<(), String> {
        if self.cluster_name.trim().is_empty() {
            return Err("cluster_name is required".to_string());
        }

        if self.mode == ClusterMode::Create {
            if self.cluster_spec.is_none() {
                return Err("cluster_spec is required when mode is 'create'".to_string());
            }
            if self.service_name.is_none()
                || self.service_name.as_ref().is_some_and(|s| s.trim().is_empty())
            {
                return Err("service_name is required when mode is 'create'".to_string());
            }
        }

        Ok(())
    }
}

/// Query parameters for listing filters
#[derive(Debug, Deserialize, ToSchema)]
pub struct ListFiltersQuery {
    #[serde(default)]
    pub limit: Option<i32>,
    #[serde(default)]
    pub offset: Option<i32>,
}

/// Request body for creating a filter
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateFilterRequest {
    pub name: String,
    pub filter_type: FilterType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub config: FilterConfig,
    pub team: String,
    /// Optional cluster configuration for filters that require backend clusters
    /// (OAuth2, JWT Auth, ExtAuthz). When mode is "create", a new cluster will
    /// be created before the filter. When mode is "reuse", an existing cluster
    /// will be validated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster_config: Option<ClusterCreationConfig>,
}

/// Request body for updating a filter
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateFilterRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<FilterConfig>,
}

/// Response body for filter operations
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FilterResponse {
    pub id: String,
    pub name: String,
    pub filter_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub config: FilterConfig,
    pub version: i64,
    pub source: String,
    pub team: String,
    pub created_at: String,
    pub updated_at: String,
    /// Valid attachment points for this filter type
    pub allowed_attachment_points: Vec<AttachmentPoint>,
    /// Number of resources this filter is attached to (routes + listeners)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachment_count: Option<i64>,
}

impl FilterResponse {
    /// Convert from FilterData and parsed config
    pub fn from_data(data: FilterData, config: FilterConfig) -> Self {
        Self::from_data_with_count(data, config, None)
    }

    /// Convert from FilterData and parsed config with attachment count
    pub fn from_data_with_count(
        data: FilterData,
        config: FilterConfig,
        attachment_count: Option<i64>,
    ) -> Self {
        // Parse filter type to get allowed attachment points
        let allowed_attachment_points = data
            .filter_type
            .parse::<FilterType>()
            .map(|ft| ft.allowed_attachment_points())
            .unwrap_or_default();

        Self {
            id: data.id.to_string(),
            name: data.name,
            filter_type: data.filter_type,
            description: data.description,
            config,
            version: data.version,
            source: data.source,
            team: data.team,
            created_at: data.created_at.to_rfc3339(),
            updated_at: data.updated_at.to_rfc3339(),
            allowed_attachment_points,
            attachment_count,
        }
    }
}

/// Request body for attaching a filter to a route
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AttachFilterRequest {
    pub filter_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<i64>,
}

/// Response for listing route filters
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RouteFiltersResponse {
    pub route_id: String,
    pub filters: Vec<FilterResponse>,
}

/// Response for listing listener filters
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListenerFiltersResponse {
    pub listener_id: String,
    pub filters: Vec<FilterResponse>,
}

// ============================================================================
// Install/Configure Types (Filter Install/Configure Redesign)
// ============================================================================

// Re-export repository types for API use
pub use crate::storage::{FilterConfiguration, FilterInstallation, FilterScopeType};

/// Scope type for filter configuration (API type with utoipa support)
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ScopeType {
    /// Apply to entire route configuration
    RouteConfig,
    /// Apply to specific virtual host
    VirtualHost,
    /// Apply to specific route
    Route,
}

impl std::fmt::Display for ScopeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScopeType::RouteConfig => write!(f, "route-config"),
            ScopeType::VirtualHost => write!(f, "virtual-host"),
            ScopeType::Route => write!(f, "route"),
        }
    }
}

impl std::str::FromStr for ScopeType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "route-config" => Ok(ScopeType::RouteConfig),
            "virtual-host" => Ok(ScopeType::VirtualHost),
            "route" => Ok(ScopeType::Route),
            _ => Err(format!("Invalid scope type: {}", s)),
        }
    }
}

impl From<FilterScopeType> for ScopeType {
    fn from(t: FilterScopeType) -> Self {
        match t {
            FilterScopeType::RouteConfig => ScopeType::RouteConfig,
            FilterScopeType::VirtualHost => ScopeType::VirtualHost,
            FilterScopeType::Route => ScopeType::Route,
        }
    }
}

impl From<ScopeType> for FilterScopeType {
    fn from(t: ScopeType) -> Self {
        match t {
            ScopeType::RouteConfig => FilterScopeType::RouteConfig,
            ScopeType::VirtualHost => FilterScopeType::VirtualHost,
            ScopeType::Route => FilterScopeType::Route,
        }
    }
}

/// Request body for installing a filter on a listener
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InstallFilterRequest {
    /// Name of the listener to install the filter on
    pub listener_name: String,
    /// Optional execution order (lower numbers execute first)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<i64>,
}

/// Response for installing a filter
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InstallFilterResponse {
    pub filter_id: String,
    pub listener_id: String,
    pub listener_name: String,
    pub order: i64,
}

/// Single installation item in list response
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FilterInstallationItem {
    pub listener_id: String,
    pub listener_name: String,
    pub listener_address: String,
    pub order: i64,
}

impl From<FilterInstallation> for FilterInstallationItem {
    fn from(f: FilterInstallation) -> Self {
        FilterInstallationItem {
            listener_id: f.listener_id,
            listener_name: f.listener_name,
            listener_address: f.listener_address,
            order: f.order,
        }
    }
}

/// Response for listing filter installations
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FilterInstallationsResponse {
    pub filter_id: String,
    pub filter_name: String,
    pub installations: Vec<FilterInstallationItem>,
}

/// Request body for configuring filter scope
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConfigureFilterRequest {
    /// Type of scope: "route-config", "virtual-host", or "route"
    pub scope_type: ScopeType,
    /// ID or name of the scope resource
    pub scope_id: String,
    /// Optional per-route/vhost settings (e.g., disabled: true, or override config)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings: Option<serde_json::Value>,
}

/// Response for configuring a filter
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConfigureFilterResponse {
    pub filter_id: String,
    pub scope_type: ScopeType,
    pub scope_id: String,
    pub scope_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings: Option<serde_json::Value>,
}

/// Single configuration item in list response
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FilterConfigurationItem {
    pub scope_type: ScopeType,
    pub scope_id: String,
    pub scope_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings: Option<serde_json::Value>,
}

impl From<FilterConfiguration> for FilterConfigurationItem {
    fn from(f: FilterConfiguration) -> Self {
        FilterConfigurationItem {
            scope_type: f.scope_type.into(),
            scope_id: f.scope_id,
            scope_name: f.scope_name,
            settings: f.settings,
        }
    }
}

/// Response for listing filter configurations
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FilterConfigurationsResponse {
    pub filter_id: String,
    pub filter_name: String,
    pub configurations: Vec<FilterConfigurationItem>,
}

/// Combined status response showing all installations and configurations
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FilterStatusResponse {
    pub filter_id: String,
    pub filter_name: String,
    pub filter_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub installations: Vec<FilterInstallationItem>,
    pub configurations: Vec<FilterConfigurationItem>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xds::EndpointSpec;

    fn create_valid_cluster_spec() -> ClusterSpec {
        ClusterSpec {
            endpoints: vec![EndpointSpec::String("auth.example.com:443".to_string())],
            use_tls: Some(true),
            ..Default::default()
        }
    }

    #[test]
    fn test_cluster_mode_default_is_reuse() {
        assert_eq!(ClusterMode::default(), ClusterMode::Reuse);
    }

    #[test]
    fn test_cluster_creation_config_reuse_mode_valid() {
        let config = ClusterCreationConfig {
            mode: ClusterMode::Reuse,
            cluster_name: "existing-cluster".to_string(),
            service_name: None,
            cluster_spec: None,
            team: None,
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_cluster_creation_config_create_mode_valid() {
        let config = ClusterCreationConfig {
            mode: ClusterMode::Create,
            cluster_name: "new-oauth-cluster".to_string(),
            service_name: Some("oauth-provider".to_string()),
            cluster_spec: Some(create_valid_cluster_spec()),
            team: Some("test-team".to_string()),
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_cluster_creation_config_empty_name_fails() {
        let config = ClusterCreationConfig {
            mode: ClusterMode::Reuse,
            cluster_name: "".to_string(),
            service_name: None,
            cluster_spec: None,
            team: None,
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("cluster_name is required"));
    }

    #[test]
    fn test_cluster_creation_config_whitespace_name_fails() {
        let config = ClusterCreationConfig {
            mode: ClusterMode::Reuse,
            cluster_name: "   ".to_string(),
            service_name: None,
            cluster_spec: None,
            team: None,
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("cluster_name is required"));
    }

    #[test]
    fn test_cluster_creation_config_create_mode_missing_spec_fails() {
        let config = ClusterCreationConfig {
            mode: ClusterMode::Create,
            cluster_name: "new-cluster".to_string(),
            service_name: Some("my-service".to_string()),
            cluster_spec: None, // Missing spec
            team: None,
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("cluster_spec is required"));
    }

    #[test]
    fn test_cluster_creation_config_create_mode_missing_service_name_fails() {
        let config = ClusterCreationConfig {
            mode: ClusterMode::Create,
            cluster_name: "new-cluster".to_string(),
            service_name: None, // Missing service name
            cluster_spec: Some(create_valid_cluster_spec()),
            team: None,
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("service_name is required"));
    }

    #[test]
    fn test_cluster_creation_config_create_mode_empty_service_name_fails() {
        let config = ClusterCreationConfig {
            mode: ClusterMode::Create,
            cluster_name: "new-cluster".to_string(),
            service_name: Some("  ".to_string()), // Empty/whitespace service name
            cluster_spec: Some(create_valid_cluster_spec()),
            team: None,
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("service_name is required"));
    }

    #[test]
    fn test_cluster_creation_config_reuse_mode_ignores_spec() {
        // In reuse mode, cluster_spec should be ignored even if provided
        let config = ClusterCreationConfig {
            mode: ClusterMode::Reuse,
            cluster_name: "existing-cluster".to_string(),
            service_name: None,
            cluster_spec: Some(create_valid_cluster_spec()), // Provided but ignored
            team: None,
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_cluster_mode_serialization() {
        let create_mode = ClusterMode::Create;
        let reuse_mode = ClusterMode::Reuse;

        assert_eq!(serde_json::to_string(&create_mode).unwrap(), "\"create\"");
        assert_eq!(serde_json::to_string(&reuse_mode).unwrap(), "\"reuse\"");
    }

    #[test]
    fn test_cluster_mode_deserialization() {
        let create_mode: ClusterMode = serde_json::from_str("\"create\"").unwrap();
        let reuse_mode: ClusterMode = serde_json::from_str("\"reuse\"").unwrap();

        assert_eq!(create_mode, ClusterMode::Create);
        assert_eq!(reuse_mode, ClusterMode::Reuse);
    }

    #[test]
    fn test_cluster_creation_config_json_roundtrip() {
        let config = ClusterCreationConfig {
            mode: ClusterMode::Create,
            cluster_name: "oauth-cluster".to_string(),
            service_name: Some("oauth-provider".to_string()),
            cluster_spec: Some(create_valid_cluster_spec()),
            team: Some("my-team".to_string()),
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: ClusterCreationConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.mode, config.mode);
        assert_eq!(deserialized.cluster_name, config.cluster_name);
        assert_eq!(deserialized.service_name, config.service_name);
        assert_eq!(deserialized.team, config.team);
    }
}
