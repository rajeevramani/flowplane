use crate::xds::filters::http::compressor::CompressorConfig;
use crate::xds::filters::http::cors::CorsConfig;
use crate::xds::filters::http::custom_response::CustomResponseConfig;
use crate::xds::filters::http::ext_authz::ExtAuthzConfig;
use crate::xds::filters::http::jwt_auth::JwtAuthenticationConfig;
use crate::xds::filters::http::local_rate_limit::LocalRateLimitConfig;
use crate::xds::filters::http::mcp::McpFilterConfig;
use crate::xds::filters::http::oauth2::OAuth2Config;
use crate::xds::filters::http::rbac::RbacConfig;
use serde::{Deserialize, Serialize};
use std::fmt;
use utoipa::ToSchema;

/// Describes how a filter type handles per-route configuration.
///
/// Different Envoy filters have varying levels of per-route customization support:
/// - Some can be fully overridden per route (e.g., HeaderMutation with full config)
/// - Some only allow referencing pre-defined configs (e.g., JWT with requirement_name)
/// - Some can only be disabled (e.g., generic FilterConfig with disabled flag)
/// - Some have no per-route support at all
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PerRouteBehavior {
    /// Full configuration override at per-route level (HeaderMutation, LocalRateLimit, CustomResponse)
    #[default]
    FullConfig,
    /// Reference to listener-level config by name (JwtAuth with requirement_name)
    ReferenceOnly,
    /// Only supports disabling the filter per-route (generic FilterConfig disabled flag)
    DisableOnly,
    /// No per-route configuration support
    NotSupported,
}

/// Static metadata about a filter type's capabilities.
///
/// This struct provides a single source of truth for all filter type metadata,
/// eliminating scattered match statements across the codebase. Filter metadata
/// is tied to Envoy protobuf definitions and provides compile-time type safety.
#[derive(Debug, Clone)]
pub struct FilterTypeMetadata {
    /// The filter type this metadata describes
    pub filter_type: FilterType,
    /// Envoy HTTP filter name (e.g., "envoy.filters.http.header_mutation")
    pub http_filter_name: &'static str,
    /// Full protobuf type URL for listener-level configuration
    pub type_url: &'static str,
    /// Full protobuf type URL for per-route configuration (if supported)
    pub per_route_type_url: Option<&'static str>,
    /// Valid attachment points for this filter
    pub attachment_points: &'static [AttachmentPoint],
    /// Whether this filter requires listener-level configuration
    pub requires_listener_config: bool,
    /// How this filter handles per-route configuration
    pub per_route_behavior: PerRouteBehavior,
    /// Whether this filter type has full implementation support
    pub is_implemented: bool,
    /// Human-readable description of the filter
    pub description: &'static str,
}

/// Static attachment point arrays for filter metadata.
/// All HTTP filters require listener installation (HCM chain) and support route configuration.
const ROUTE_AND_LISTENER: &[AttachmentPoint] = &[AttachmentPoint::Route, AttachmentPoint::Listener];

/// Returns metadata for a given filter type.
///
/// This is the central registry of filter metadata, providing a single source
/// of truth for all filter capabilities and configuration.
fn filter_registry(filter_type: FilterType) -> FilterTypeMetadata {
    match filter_type {
        FilterType::HeaderMutation => FilterTypeMetadata {
            filter_type: FilterType::HeaderMutation,
            http_filter_name: "envoy.filters.http.header_mutation",
            type_url: "type.googleapis.com/envoy.extensions.filters.http.header_mutation.v3.HeaderMutation",
            per_route_type_url: Some("type.googleapis.com/envoy.extensions.filters.http.header_mutation.v3.HeaderMutationPerRoute"),
            attachment_points: ROUTE_AND_LISTENER,
            requires_listener_config: false,
            per_route_behavior: PerRouteBehavior::FullConfig,
            is_implemented: true,
            description: "Add, modify, or remove HTTP headers",
        },
        FilterType::JwtAuth => FilterTypeMetadata {
            filter_type: FilterType::JwtAuth,
            http_filter_name: "envoy.filters.http.jwt_authn",
            type_url: "type.googleapis.com/envoy.extensions.filters.http.jwt_authn.v3.JwtAuthentication",
            per_route_type_url: Some("type.googleapis.com/envoy.extensions.filters.http.jwt_authn.v3.PerRouteConfig"),
            attachment_points: ROUTE_AND_LISTENER,
            requires_listener_config: true,
            per_route_behavior: PerRouteBehavior::ReferenceOnly,
            is_implemented: true,
            description: "JSON Web Token authentication",
        },
        FilterType::Cors => FilterTypeMetadata {
            filter_type: FilterType::Cors,
            http_filter_name: "envoy.filters.http.cors",
            type_url: "type.googleapis.com/envoy.extensions.filters.http.cors.v3.Cors",
            per_route_type_url: Some("type.googleapis.com/envoy.extensions.filters.http.cors.v3.CorsPolicy"),
            attachment_points: ROUTE_AND_LISTENER,
            requires_listener_config: false,
            per_route_behavior: PerRouteBehavior::FullConfig,
            is_implemented: true,
            description: "Cross-Origin Resource Sharing policy",
        },
        FilterType::Compressor => FilterTypeMetadata {
            filter_type: FilterType::Compressor,
            http_filter_name: "envoy.filters.http.compressor",
            type_url: "type.googleapis.com/envoy.extensions.filters.http.compressor.v3.Compressor",
            per_route_type_url: Some("type.googleapis.com/envoy.extensions.filters.http.compressor.v3.CompressorPerRoute"),
            attachment_points: ROUTE_AND_LISTENER,
            requires_listener_config: true,
            per_route_behavior: PerRouteBehavior::DisableOnly,
            is_implemented: true,
            description: "Response compression filter (gzip)",
        },
        FilterType::LocalRateLimit => FilterTypeMetadata {
            filter_type: FilterType::LocalRateLimit,
            http_filter_name: "envoy.filters.http.local_ratelimit",
            type_url: "type.googleapis.com/envoy.extensions.filters.http.local_ratelimit.v3.LocalRateLimit",
            per_route_type_url: Some("type.googleapis.com/envoy.extensions.filters.http.local_ratelimit.v3.LocalRateLimit"),
            attachment_points: ROUTE_AND_LISTENER,
            requires_listener_config: true,
            per_route_behavior: PerRouteBehavior::FullConfig,
            is_implemented: true,
            description: "Local (in-memory) rate limiting",
        },
        FilterType::RateLimit => FilterTypeMetadata {
            filter_type: FilterType::RateLimit,
            http_filter_name: "envoy.filters.http.ratelimit",
            type_url: "type.googleapis.com/envoy.extensions.filters.http.ratelimit.v3.RateLimit",
            per_route_type_url: Some("type.googleapis.com/envoy.extensions.filters.http.ratelimit.v3.RateLimitPerRoute"),
            attachment_points: ROUTE_AND_LISTENER,
            requires_listener_config: true,
            per_route_behavior: PerRouteBehavior::FullConfig,
            is_implemented: false,
            description: "External/distributed rate limiting (requires gRPC service)",
        },
        FilterType::ExtAuthz => FilterTypeMetadata {
            filter_type: FilterType::ExtAuthz,
            http_filter_name: "envoy.filters.http.ext_authz",
            type_url: "type.googleapis.com/envoy.extensions.filters.http.ext_authz.v3.ExtAuthz",
            per_route_type_url: Some("type.googleapis.com/envoy.extensions.filters.http.ext_authz.v3.ExtAuthzPerRoute"),
            attachment_points: ROUTE_AND_LISTENER,
            requires_listener_config: true,
            per_route_behavior: PerRouteBehavior::FullConfig,
            is_implemented: true,
            description: "External authorization service",
        },
        FilterType::Rbac => FilterTypeMetadata {
            filter_type: FilterType::Rbac,
            http_filter_name: "envoy.filters.http.rbac",
            type_url: "type.googleapis.com/envoy.extensions.filters.http.rbac.v3.RBAC",
            per_route_type_url: Some("type.googleapis.com/envoy.extensions.filters.http.rbac.v3.RBACPerRoute"),
            attachment_points: ROUTE_AND_LISTENER,
            requires_listener_config: true,
            per_route_behavior: PerRouteBehavior::FullConfig,
            is_implemented: true,
            description: "Role-based access control",
        },
        FilterType::OAuth2 => FilterTypeMetadata {
            filter_type: FilterType::OAuth2,
            http_filter_name: "envoy.filters.http.oauth2",
            type_url: "type.googleapis.com/envoy.extensions.filters.http.oauth2.v3.OAuth2",
            // OAuth2 does NOT support typed_per_filter_config at all
            // Envoy error: "The filter envoy.filters.http.oauth2 doesn't support virtual host or route specific configurations"
            per_route_type_url: None,
            attachment_points: &[AttachmentPoint::Listener],
            requires_listener_config: true,
            per_route_behavior: PerRouteBehavior::NotSupported,
            is_implemented: true,
            description: "OAuth2 authentication filter",
        },
        FilterType::CustomResponse => FilterTypeMetadata {
            filter_type: FilterType::CustomResponse,
            http_filter_name: "envoy.filters.http.custom_response",
            type_url: "type.googleapis.com/envoy.extensions.filters.http.custom_response.v3.CustomResponse",
            per_route_type_url: Some("type.googleapis.com/envoy.extensions.filters.http.custom_response.v3.CustomResponse"),
            attachment_points: ROUTE_AND_LISTENER,
            requires_listener_config: true,
            per_route_behavior: PerRouteBehavior::FullConfig,
            is_implemented: true,
            description: "Custom response filter for modifying responses based on status codes",
        },
        FilterType::Mcp => FilterTypeMetadata {
            filter_type: FilterType::Mcp,
            http_filter_name: "envoy.filters.http.mcp",
            type_url: "type.googleapis.com/envoy.extensions.filters.http.mcp.v3.Mcp",
            per_route_type_url: Some("type.googleapis.com/envoy.extensions.filters.http.mcp.v3.Mcp"),
            attachment_points: ROUTE_AND_LISTENER,
            requires_listener_config: true,
            per_route_behavior: PerRouteBehavior::DisableOnly,
            is_implemented: true,
            description: "Model Context Protocol filter for AI/LLM gateway traffic",
        },
    }
}

/// Represents the resource types that a filter can attach to.
/// Different filter types have different valid attachment points based on their scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, Hash)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentPoint {
    /// Route-level attachment (per-route filter config via typed_per_filter_config)
    Route,
    /// Listener-level attachment (HTTP connection manager filter chain)
    Listener,
    /// Cluster-level attachment (future: connection pool, health check, outlier detection)
    Cluster,
}

impl fmt::Display for AttachmentPoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AttachmentPoint::Route => write!(f, "route"),
            AttachmentPoint::Listener => write!(f, "listener"),
            AttachmentPoint::Cluster => write!(f, "cluster"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FilterType {
    HeaderMutation,
    JwtAuth,
    Cors,
    /// Response compression (gzip)
    Compressor,
    /// Local (in-memory) rate limiting
    LocalRateLimit,
    /// External/distributed rate limiting (requires gRPC service)
    RateLimit,
    ExtAuthz,
    /// Role-based access control
    Rbac,
    /// OAuth2 authentication
    #[serde(rename = "oauth2")]
    OAuth2,
    /// Custom response filter for modifying responses based on status codes
    CustomResponse,
    /// Model Context Protocol (MCP) filter for AI/LLM gateway traffic
    /// Inspects and validates JSON-RPC 2.0 and SSE stream traffic
    Mcp,
}

impl FilterType {
    /// Returns the static metadata for this filter type.
    ///
    /// This provides a single source of truth for all filter capabilities
    /// and configuration details.
    pub fn metadata(&self) -> FilterTypeMetadata {
        filter_registry(*self)
    }

    /// Returns the valid attachment points for this filter type.
    ///
    /// Filter types have different scopes:
    /// - HeaderMutation, Cors: Route-level only (L7 HTTP route filters)
    /// - JwtAuth, RateLimit, ExtAuthz, CustomResponse: Can apply at both route and listener levels
    pub fn allowed_attachment_points(&self) -> Vec<AttachmentPoint> {
        self.metadata().attachment_points.to_vec()
    }

    /// Checks if this filter type can attach to the given attachment point.
    pub fn can_attach_to(&self, point: AttachmentPoint) -> bool {
        self.metadata().attachment_points.contains(&point)
    }

    /// Returns a human-readable description of allowed attachment points.
    pub fn allowed_attachment_points_display(&self) -> String {
        let points = self.metadata().attachment_points;
        if points.len() == 1 {
            format!("{} only", points[0])
        } else {
            points.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(", ")
        }
    }

    /// Returns the Envoy HTTP filter name for this filter type.
    ///
    /// This is the canonical name used in Envoy's HTTP connection manager filter chain
    /// and in `typed_per_filter_config` entries.
    pub fn http_filter_name(&self) -> &'static str {
        self.metadata().http_filter_name
    }

    /// Returns true if this filter type has full implementation support.
    ///
    /// Used for API validation to reject unsupported filter creation.
    /// Filter types that are defined but not yet fully implemented will return false.
    pub fn is_fully_implemented(&self) -> bool {
        self.metadata().is_implemented
    }

    /// Returns whether this filter type requires listener-level configuration.
    ///
    /// Filters requiring listener config cannot be created as empty placeholders
    /// in the HCM filter chain - they need their full configuration attached.
    ///
    /// - JwtAuth: Requires providers and requirement_map
    /// - LocalRateLimit: Requires token_bucket configuration
    /// - RateLimit: Requires gRPC service configuration
    /// - ExtAuthz: Requires service configuration
    /// - CustomResponse: Requires matcher configuration
    /// - Mcp: Requires traffic_mode configuration
    ///
    /// Filters that work as placeholders (HeaderMutation, Cors) return false.
    pub fn requires_listener_config(&self) -> bool {
        self.metadata().requires_listener_config
    }

    /// Returns the per-route behavior for this filter type.
    ///
    /// Describes how this filter handles per-route configuration overrides.
    pub fn per_route_behavior(&self) -> PerRouteBehavior {
        self.metadata().per_route_behavior
    }

    /// Looks up a filter type by its Envoy HTTP filter name.
    ///
    /// Returns `None` if no filter type matches the given name.
    pub fn from_http_filter_name(name: &str) -> Option<Self> {
        [
            FilterType::HeaderMutation,
            FilterType::JwtAuth,
            FilterType::Cors,
            FilterType::Compressor,
            FilterType::LocalRateLimit,
            FilterType::RateLimit,
            FilterType::ExtAuthz,
            FilterType::Rbac,
            FilterType::OAuth2,
            FilterType::CustomResponse,
            FilterType::Mcp,
        ]
        .into_iter()
        .find(|filter_type| filter_type.http_filter_name() == name)
    }

    /// Create a FilterType from a string, supporting both built-in and dynamic types.
    ///
    /// This is used by the dynamic schema system to convert schema names to FilterType.
    /// For built-in types, returns the corresponding enum variant.
    /// For unknown types, returns the first matching built-in type or falls back to HeaderMutation.
    ///
    /// Note: In a fully dynamic system, this would return a Dynamic(String) variant,
    /// but we maintain backward compatibility with the existing enum-based system.
    pub fn from_str_dynamic(s: &str) -> Self {
        s.parse().unwrap_or(FilterType::HeaderMutation)
    }
}

impl fmt::Display for FilterType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FilterType::HeaderMutation => write!(f, "header_mutation"),
            FilterType::JwtAuth => write!(f, "jwt_auth"),
            FilterType::Cors => write!(f, "cors"),
            FilterType::Compressor => write!(f, "compressor"),
            FilterType::LocalRateLimit => write!(f, "local_rate_limit"),
            FilterType::RateLimit => write!(f, "rate_limit"),
            FilterType::ExtAuthz => write!(f, "ext_authz"),
            FilterType::Rbac => write!(f, "rbac"),
            FilterType::OAuth2 => write!(f, "oauth2"),
            FilterType::CustomResponse => write!(f, "custom_response"),
            FilterType::Mcp => write!(f, "mcp"),
        }
    }
}

impl std::str::FromStr for FilterType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "header_mutation" => Ok(FilterType::HeaderMutation),
            "jwt_auth" => Ok(FilterType::JwtAuth),
            "cors" => Ok(FilterType::Cors),
            "compressor" => Ok(FilterType::Compressor),
            "local_rate_limit" => Ok(FilterType::LocalRateLimit),
            "rate_limit" => Ok(FilterType::RateLimit),
            "ext_authz" => Ok(FilterType::ExtAuthz),
            "rbac" => Ok(FilterType::Rbac),
            "oauth2" => Ok(FilterType::OAuth2),
            "custom_response" => Ok(FilterType::CustomResponse),
            "mcp" => Ok(FilterType::Mcp),
            _ => Err(format!("Unknown filter type: {}", s)),
        }
    }
}

/// Check if a filter type string can attach to a given attachment point.
///
/// This handles both built-in filter types (with known attachment rules)
/// and custom filter types (which default to allowing Route and Listener attachment).
///
/// Returns Ok(()) if attachment is allowed, or Err with a descriptive message if not.
pub fn can_filter_type_attach_to(
    filter_type_str: &str,
    attachment_point: AttachmentPoint,
) -> Result<(), String> {
    // Try to parse as a built-in type
    if let Ok(filter_type) = filter_type_str.parse::<FilterType>() {
        if filter_type.can_attach_to(attachment_point) {
            return Ok(());
        }
        return Err(format!(
            "Filter type '{}' cannot be attached to {}. Valid attachment points: {}",
            filter_type_str,
            attachment_point,
            filter_type.allowed_attachment_points_display()
        ));
    }

    // Custom filter types (wasm, lua, etc.) - allow Route and Listener by default
    // Cluster attachment is not supported for custom HTTP filters
    match attachment_point {
        AttachmentPoint::Route | AttachmentPoint::Listener => Ok(()),
        AttachmentPoint::Cluster => {
            Err(format!("Custom filter type '{}' cannot be attached to clusters", filter_type_str))
        }
    }
}

/// Get the allowed attachment points for a filter type string.
///
/// Returns the attachment points for built-in types, or Route + Listener for custom types.
pub fn get_filter_type_attachment_points(filter_type_str: &str) -> Vec<AttachmentPoint> {
    if let Ok(filter_type) = filter_type_str.parse::<FilterType>() {
        filter_type.allowed_attachment_points()
    } else {
        // Custom types default to Route + Listener
        vec![AttachmentPoint::Route, AttachmentPoint::Listener]
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct HeaderMutationEntry {
    pub key: String,
    pub value: String,
    #[serde(default)]
    pub append: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct HeaderMutationFilterConfig {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub request_headers_to_add: Vec<HeaderMutationEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub request_headers_to_remove: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub response_headers_to_add: Vec<HeaderMutationEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub response_headers_to_remove: Vec<String>,
}

/// Configuration for custom/dynamic filter types loaded from schema registry.
///
/// This allows creating filters from YAML schema definitions without
/// requiring compile-time Rust code.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CustomFilterConfig {
    /// The filter type name (e.g., "wasm", "lua")
    /// Must match a schema in the filter-schemas directory
    #[serde(rename = "type")]
    pub filter_type: String,
    /// The filter configuration as JSON
    /// Validated against the schema's config_schema
    pub config: serde_json::Value,
}

/// Envelope for all filter configurations (extensible for future filter types)
///
/// This enum uses custom Serialize/Deserialize implementations to support both
/// built-in filter types (with typed configs) and custom/dynamic filter types
/// (with arbitrary JSON configs loaded from schema registry).
#[derive(Debug, Clone, ToSchema)]
pub enum FilterConfig {
    HeaderMutation(HeaderMutationFilterConfig),
    JwtAuth(JwtAuthenticationConfig),
    LocalRateLimit(LocalRateLimitConfig),
    CustomResponse(CustomResponseConfig),
    /// Model Context Protocol (MCP) filter configuration
    Mcp(McpFilterConfig),
    /// CORS filter configuration
    Cors(CorsConfig),
    /// Response compression filter configuration
    Compressor(CompressorConfig),
    /// External authorization filter configuration
    ExtAuthz(ExtAuthzConfig),
    /// Role-based access control filter configuration
    Rbac(RbacConfig),
    /// OAuth2 authentication filter configuration
    OAuth2(OAuth2Config),
    /// Custom/dynamic filter loaded from schema registry
    /// Used for filter types defined in filter-schemas/custom/
    Custom(CustomFilterConfig),
}

// Custom serializer that produces {"type": "...", "config": {...}} format
// and handles both built-in and custom filter types
impl Serialize for FilterConfig {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;

        let mut map = serializer.serialize_map(Some(2))?;

        match self {
            FilterConfig::HeaderMutation(config) => {
                map.serialize_entry("type", "header_mutation")?;
                map.serialize_entry("config", config)?;
            }
            FilterConfig::JwtAuth(config) => {
                map.serialize_entry("type", "jwt_auth")?;
                map.serialize_entry("config", config)?;
            }
            FilterConfig::LocalRateLimit(config) => {
                map.serialize_entry("type", "local_rate_limit")?;
                map.serialize_entry("config", config)?;
            }
            FilterConfig::CustomResponse(config) => {
                map.serialize_entry("type", "custom_response")?;
                map.serialize_entry("config", config)?;
            }
            FilterConfig::Mcp(config) => {
                map.serialize_entry("type", "mcp")?;
                map.serialize_entry("config", config)?;
            }
            FilterConfig::Cors(config) => {
                map.serialize_entry("type", "cors")?;
                map.serialize_entry("config", config)?;
            }
            FilterConfig::Compressor(config) => {
                map.serialize_entry("type", "compressor")?;
                map.serialize_entry("config", config)?;
            }
            FilterConfig::ExtAuthz(config) => {
                map.serialize_entry("type", "ext_authz")?;
                map.serialize_entry("config", config)?;
            }
            FilterConfig::Rbac(config) => {
                map.serialize_entry("type", "rbac")?;
                map.serialize_entry("config", config)?;
            }
            FilterConfig::OAuth2(config) => {
                map.serialize_entry("type", "oauth2")?;
                map.serialize_entry("config", config)?;
            }
            FilterConfig::Custom(custom) => {
                // For custom filters, use the dynamic type name from CustomFilterConfig
                map.serialize_entry("type", &custom.filter_type)?;
                map.serialize_entry("config", &custom.config)?;
            }
        }

        map.end()
    }
}

// Custom deserializer that falls back to Custom for unknown types
impl<'de> serde::Deserialize<'de> for FilterConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error;

        // First, deserialize to a raw Value
        let value = serde_json::Value::deserialize(deserializer)?;

        // Extract the type field
        let filter_type = value
            .get("type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| D::Error::missing_field("type"))?;

        // Try to match known types
        match filter_type {
            "header_mutation" => {
                let config =
                    value.get("config").ok_or_else(|| D::Error::missing_field("config"))?;
                let inner: HeaderMutationFilterConfig =
                    serde_json::from_value(config.clone()).map_err(D::Error::custom)?;
                Ok(FilterConfig::HeaderMutation(inner))
            }
            "jwt_auth" => {
                let config =
                    value.get("config").ok_or_else(|| D::Error::missing_field("config"))?;
                let inner: JwtAuthenticationConfig =
                    serde_json::from_value(config.clone()).map_err(D::Error::custom)?;
                Ok(FilterConfig::JwtAuth(inner))
            }
            "local_rate_limit" => {
                let config =
                    value.get("config").ok_or_else(|| D::Error::missing_field("config"))?;
                let inner: LocalRateLimitConfig =
                    serde_json::from_value(config.clone()).map_err(D::Error::custom)?;
                Ok(FilterConfig::LocalRateLimit(inner))
            }
            "custom_response" => {
                let config =
                    value.get("config").ok_or_else(|| D::Error::missing_field("config"))?;
                let inner: CustomResponseConfig =
                    serde_json::from_value(config.clone()).map_err(D::Error::custom)?;
                Ok(FilterConfig::CustomResponse(inner))
            }
            "mcp" => {
                let config =
                    value.get("config").ok_or_else(|| D::Error::missing_field("config"))?;
                let inner: McpFilterConfig =
                    serde_json::from_value(config.clone()).map_err(D::Error::custom)?;
                Ok(FilterConfig::Mcp(inner))
            }
            "cors" => {
                let config =
                    value.get("config").ok_or_else(|| D::Error::missing_field("config"))?;
                let inner: CorsConfig =
                    serde_json::from_value(config.clone()).map_err(D::Error::custom)?;
                Ok(FilterConfig::Cors(inner))
            }
            "compressor" => {
                let config =
                    value.get("config").ok_or_else(|| D::Error::missing_field("config"))?;
                let inner: CompressorConfig =
                    serde_json::from_value(config.clone()).map_err(D::Error::custom)?;
                Ok(FilterConfig::Compressor(inner))
            }
            "ext_authz" => {
                let config =
                    value.get("config").ok_or_else(|| D::Error::missing_field("config"))?;
                let inner: ExtAuthzConfig =
                    serde_json::from_value(config.clone()).map_err(D::Error::custom)?;
                Ok(FilterConfig::ExtAuthz(inner))
            }
            "rbac" => {
                let config =
                    value.get("config").ok_or_else(|| D::Error::missing_field("config"))?;
                let inner: RbacConfig =
                    serde_json::from_value(config.clone()).map_err(D::Error::custom)?;
                Ok(FilterConfig::Rbac(inner))
            }
            "oauth2" => {
                let config =
                    value.get("config").ok_or_else(|| D::Error::missing_field("config"))?;
                let inner: OAuth2Config =
                    serde_json::from_value(config.clone()).map_err(D::Error::custom)?;
                Ok(FilterConfig::OAuth2(inner))
            }
            // Unknown type - treat as custom/dynamic filter
            unknown_type => {
                let config = value
                    .get("config")
                    .cloned()
                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                Ok(FilterConfig::Custom(CustomFilterConfig {
                    filter_type: unknown_type.to_string(),
                    config,
                }))
            }
        }
    }
}

impl FilterConfig {
    /// Returns the filter type as a string.
    ///
    /// This works for both built-in and custom filter types.
    /// For storage and API responses, prefer this over `filter_type()`.
    pub fn filter_type_str(&self) -> String {
        match self {
            FilterConfig::HeaderMutation(_) => "header_mutation".to_string(),
            FilterConfig::JwtAuth(_) => "jwt_auth".to_string(),
            FilterConfig::LocalRateLimit(_) => "local_rate_limit".to_string(),
            FilterConfig::CustomResponse(_) => "custom_response".to_string(),
            FilterConfig::Mcp(_) => "mcp".to_string(),
            FilterConfig::Cors(_) => "cors".to_string(),
            FilterConfig::Compressor(_) => "compressor".to_string(),
            FilterConfig::ExtAuthz(_) => "ext_authz".to_string(),
            FilterConfig::Rbac(_) => "rbac".to_string(),
            FilterConfig::OAuth2(_) => "oauth2".to_string(),
            FilterConfig::Custom(c) => c.filter_type.clone(),
        }
    }

    /// Returns the FilterType enum for built-in filter types.
    ///
    /// For custom filters, this returns None since they don't have
    /// a corresponding FilterType variant.
    pub fn try_filter_type(&self) -> Option<FilterType> {
        match self {
            FilterConfig::HeaderMutation(_) => Some(FilterType::HeaderMutation),
            FilterConfig::JwtAuth(_) => Some(FilterType::JwtAuth),
            FilterConfig::LocalRateLimit(_) => Some(FilterType::LocalRateLimit),
            FilterConfig::CustomResponse(_) => Some(FilterType::CustomResponse),
            FilterConfig::Mcp(_) => Some(FilterType::Mcp),
            FilterConfig::Cors(_) => Some(FilterType::Cors),
            FilterConfig::Compressor(_) => Some(FilterType::Compressor),
            FilterConfig::ExtAuthz(_) => Some(FilterType::ExtAuthz),
            FilterConfig::Rbac(_) => Some(FilterType::Rbac),
            FilterConfig::OAuth2(_) => Some(FilterType::OAuth2),
            FilterConfig::Custom(_) => None,
        }
    }

    /// Returns the FilterType enum for built-in filter types.
    ///
    /// # Panics
    /// Panics if called on a Custom filter. Use `try_filter_type()` or
    /// `filter_type_str()` for custom-safe access.
    pub fn filter_type(&self) -> FilterType {
        self.try_filter_type().expect(
            "filter_type() called on Custom filter - use try_filter_type() or filter_type_str()",
        )
    }

    /// Returns true if this is a custom/dynamic filter type.
    pub fn is_custom(&self) -> bool {
        matches!(self, FilterConfig::Custom(_))
    }

    /// Returns the raw JSON config for custom filters.
    ///
    /// Returns None for built-in filter types.
    pub fn custom_config(&self) -> Option<&serde_json::Value> {
        match self {
            FilterConfig::Custom(c) => Some(&c.config),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::handlers::filters::CreateFilterRequest;

    #[test]
    fn test_frontend_payload_deserialization() {
        // This is what the frontend sends after our fix
        let json = r#"{
            "name": "test-filter",
            "filterType": "header_mutation",
            "config": {
                "type": "header_mutation",
                "config": {
                    "request_headers_to_add": [{"key": "X-Test", "value": "val", "append": false}]
                }
            },
            "team": "test-team"
        }"#;

        let result: Result<CreateFilterRequest, _> = serde_json::from_str(json);
        assert!(result.is_ok(), "Should parse frontend payload: {:?}", result.err());

        let req = result.unwrap();
        assert_eq!(req.name, "test-filter");
        assert_eq!(req.filter_type, "header_mutation");
        assert_eq!(req.team, "test-team");

        match req.config {
            FilterConfig::HeaderMutation(cfg) => {
                assert_eq!(cfg.request_headers_to_add.len(), 1);
                assert_eq!(cfg.request_headers_to_add[0].key, "X-Test");
            }
            _ => panic!("Expected HeaderMutation config"),
        }
    }

    #[test]
    fn test_filter_type_serialization() {
        let ft = FilterType::HeaderMutation;
        let json = serde_json::to_string(&ft).unwrap();
        assert_eq!(json, r#""header_mutation""#);

        let parsed: FilterType = serde_json::from_str(r#""header_mutation""#).unwrap();
        assert_eq!(parsed, FilterType::HeaderMutation);
    }

    #[test]
    fn test_filter_config_serialization() {
        let config = FilterConfig::HeaderMutation(HeaderMutationFilterConfig {
            request_headers_to_add: vec![HeaderMutationEntry {
                key: "X-Test".to_string(),
                value: "val".to_string(),
                append: false,
            }],
            request_headers_to_remove: vec![],
            response_headers_to_add: vec![],
            response_headers_to_remove: vec![],
        });

        let json = serde_json::to_string(&config).unwrap();
        // Should be tagged enum format
        assert!(json.contains(r#""type":"header_mutation""#), "JSON: {}", json);
        assert!(json.contains(r#""config":"#), "JSON: {}", json);
    }

    // Attachment point tests

    #[test]
    fn test_attachment_point_serialization() {
        let point = AttachmentPoint::Route;
        let json = serde_json::to_string(&point).unwrap();
        assert_eq!(json, r#""route""#);

        let parsed: AttachmentPoint = serde_json::from_str(r#""listener""#).unwrap();
        assert_eq!(parsed, AttachmentPoint::Listener);
    }

    #[test]
    fn test_attachment_point_display() {
        assert_eq!(AttachmentPoint::Route.to_string(), "route");
        assert_eq!(AttachmentPoint::Listener.to_string(), "listener");
        assert_eq!(AttachmentPoint::Cluster.to_string(), "cluster");
    }

    #[test]
    fn test_header_mutation_attaches_to_routes_and_listeners() {
        let ft = FilterType::HeaderMutation;
        assert!(ft.can_attach_to(AttachmentPoint::Route));
        assert!(ft.can_attach_to(AttachmentPoint::Listener));
        assert!(!ft.can_attach_to(AttachmentPoint::Cluster));
        assert_eq!(
            ft.allowed_attachment_points(),
            vec![AttachmentPoint::Route, AttachmentPoint::Listener]
        );
    }

    #[test]
    fn test_cors_attaches_to_routes_and_listeners() {
        let ft = FilterType::Cors;
        assert!(ft.can_attach_to(AttachmentPoint::Route));
        assert!(ft.can_attach_to(AttachmentPoint::Listener));
        assert!(!ft.can_attach_to(AttachmentPoint::Cluster));
    }

    #[test]
    fn test_jwt_auth_attaches_to_routes_and_listeners() {
        let ft = FilterType::JwtAuth;
        assert!(ft.can_attach_to(AttachmentPoint::Route));
        assert!(ft.can_attach_to(AttachmentPoint::Listener));
        assert!(!ft.can_attach_to(AttachmentPoint::Cluster));
        assert_eq!(
            ft.allowed_attachment_points(),
            vec![AttachmentPoint::Route, AttachmentPoint::Listener]
        );
    }

    #[test]
    fn test_local_rate_limit_attaches_to_routes_and_listeners() {
        let ft = FilterType::LocalRateLimit;
        assert!(ft.can_attach_to(AttachmentPoint::Route));
        assert!(ft.can_attach_to(AttachmentPoint::Listener));
        assert!(!ft.can_attach_to(AttachmentPoint::Cluster));
    }

    #[test]
    fn test_rate_limit_attaches_to_routes_and_listeners() {
        let ft = FilterType::RateLimit;
        assert!(ft.can_attach_to(AttachmentPoint::Route));
        assert!(ft.can_attach_to(AttachmentPoint::Listener));
        assert!(!ft.can_attach_to(AttachmentPoint::Cluster));
    }

    #[test]
    fn test_ext_authz_attaches_to_routes_and_listeners() {
        let ft = FilterType::ExtAuthz;
        assert!(ft.can_attach_to(AttachmentPoint::Route));
        assert!(ft.can_attach_to(AttachmentPoint::Listener));
        assert!(!ft.can_attach_to(AttachmentPoint::Cluster));
    }

    #[test]
    fn test_allowed_attachment_points_display() {
        assert_eq!(
            FilterType::HeaderMutation.allowed_attachment_points_display(),
            "route, listener"
        );
        assert_eq!(FilterType::JwtAuth.allowed_attachment_points_display(), "route, listener");
    }

    #[test]
    fn test_filter_type_from_str() {
        assert_eq!("header_mutation".parse::<FilterType>().unwrap(), FilterType::HeaderMutation);
        assert_eq!("jwt_auth".parse::<FilterType>().unwrap(), FilterType::JwtAuth);
        assert_eq!("cors".parse::<FilterType>().unwrap(), FilterType::Cors);
        assert_eq!("local_rate_limit".parse::<FilterType>().unwrap(), FilterType::LocalRateLimit);
        assert_eq!("rate_limit".parse::<FilterType>().unwrap(), FilterType::RateLimit);
        assert_eq!("ext_authz".parse::<FilterType>().unwrap(), FilterType::ExtAuthz);

        // Unknown type should error
        assert!("unknown".parse::<FilterType>().is_err());
    }

    #[test]
    fn test_http_filter_name_returns_correct_envoy_names() {
        assert_eq!(
            FilterType::HeaderMutation.http_filter_name(),
            "envoy.filters.http.header_mutation"
        );
        assert_eq!(FilterType::JwtAuth.http_filter_name(), "envoy.filters.http.jwt_authn");
        assert_eq!(FilterType::Cors.http_filter_name(), "envoy.filters.http.cors");
        assert_eq!(
            FilterType::LocalRateLimit.http_filter_name(),
            "envoy.filters.http.local_ratelimit"
        );
        assert_eq!(FilterType::RateLimit.http_filter_name(), "envoy.filters.http.ratelimit");
        assert_eq!(FilterType::ExtAuthz.http_filter_name(), "envoy.filters.http.ext_authz");
    }

    #[test]
    fn test_is_fully_implemented() {
        // Currently implemented filters
        assert!(FilterType::HeaderMutation.is_fully_implemented());
        assert!(FilterType::JwtAuth.is_fully_implemented());
        assert!(FilterType::LocalRateLimit.is_fully_implemented());
        assert!(FilterType::CustomResponse.is_fully_implemented());
        assert!(FilterType::Cors.is_fully_implemented());
        assert!(FilterType::Compressor.is_fully_implemented());
        assert!(FilterType::ExtAuthz.is_fully_implemented());
        assert!(FilterType::Rbac.is_fully_implemented());
        assert!(FilterType::OAuth2.is_fully_implemented());
        assert!(FilterType::Mcp.is_fully_implemented());

        // Not yet implemented filters
        assert!(!FilterType::RateLimit.is_fully_implemented());
    }

    #[test]
    fn test_requires_listener_config() {
        // Filters that require listener-level configuration (cannot be empty placeholders)
        assert!(FilterType::JwtAuth.requires_listener_config());
        assert!(FilterType::LocalRateLimit.requires_listener_config());
        assert!(FilterType::RateLimit.requires_listener_config());
        assert!(FilterType::ExtAuthz.requires_listener_config());
        assert!(FilterType::CustomResponse.requires_listener_config());
        assert!(FilterType::Compressor.requires_listener_config());
        assert!(FilterType::Rbac.requires_listener_config());
        assert!(FilterType::OAuth2.requires_listener_config());
        assert!(FilterType::Mcp.requires_listener_config());

        // Filters that work as empty placeholders in HCM filter chain
        assert!(!FilterType::HeaderMutation.requires_listener_config());
        assert!(!FilterType::Cors.requires_listener_config());
    }

    #[test]
    fn test_jwt_auth_filter_config_filter_type() {
        use crate::xds::filters::http::jwt_auth::{
            JwtAuthenticationConfig, JwtJwksSourceConfig, JwtProviderConfig, LocalJwksConfig,
        };
        use std::collections::HashMap;

        // Create a minimal JwtAuth config
        let mut providers = HashMap::new();
        providers.insert(
            "test-provider".to_string(),
            JwtProviderConfig {
                issuer: Some("https://issuer.example.com".to_string()),
                audiences: vec!["api".to_string()],
                jwks: JwtJwksSourceConfig::Local(LocalJwksConfig {
                    inline_string: Some("{\"keys\":[]}".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        );

        let config =
            FilterConfig::JwtAuth(JwtAuthenticationConfig { providers, ..Default::default() });

        assert_eq!(config.filter_type(), FilterType::JwtAuth);
    }

    #[test]
    fn test_jwt_auth_filter_config_serialization() {
        use crate::xds::filters::http::jwt_auth::{
            JwtAuthenticationConfig, JwtJwksSourceConfig, JwtProviderConfig, LocalJwksConfig,
        };
        use std::collections::HashMap;

        // Create a minimal JwtAuth config
        let mut providers = HashMap::new();
        providers.insert(
            "test-provider".to_string(),
            JwtProviderConfig {
                issuer: Some("https://issuer.example.com".to_string()),
                audiences: vec!["api".to_string()],
                jwks: JwtJwksSourceConfig::Local(LocalJwksConfig {
                    inline_string: Some("{\"keys\":[]}".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        );

        let config =
            FilterConfig::JwtAuth(JwtAuthenticationConfig { providers, ..Default::default() });

        let json = serde_json::to_string(&config).unwrap();
        // Should be tagged enum format
        assert!(json.contains(r#""type":"jwt_auth""#), "JSON: {}", json);
        assert!(json.contains(r#""config":"#), "JSON: {}", json);
        assert!(json.contains("test-provider"), "JSON: {}", json);

        // Round-trip test
        let parsed: FilterConfig = serde_json::from_str(&json).unwrap();
        match parsed {
            FilterConfig::JwtAuth(jwt_config) => {
                assert!(jwt_config.providers.contains_key("test-provider"));
            }
            _ => panic!("Expected JwtAuth config"),
        }
    }

    #[test]
    fn test_local_rate_limit_filter_config_serialization() {
        use crate::xds::filters::http::local_rate_limit::{
            LocalRateLimitConfig, TokenBucketConfig,
        };

        let config = FilterConfig::LocalRateLimit(LocalRateLimitConfig {
            stat_prefix: "ingress_http".to_string(),
            token_bucket: Some(TokenBucketConfig {
                max_tokens: 100,
                tokens_per_fill: Some(50),
                fill_interval_ms: 1000,
            }),
            status_code: Some(429),
            filter_enabled: None,
            filter_enforced: None,
            per_downstream_connection: Some(false),
            rate_limited_as_resource_exhausted: Some(true),
            max_dynamic_descriptors: None,
            always_consume_default_token_bucket: None,
        });

        let json = serde_json::to_string(&config).unwrap();
        // Should be tagged enum format
        assert!(json.contains(r#""type":"local_rate_limit""#), "JSON: {}", json);
        assert!(json.contains(r#""config":"#), "JSON: {}", json);
        assert!(json.contains("ingress_http"), "JSON: {}", json);

        // Round-trip test
        let parsed: FilterConfig = serde_json::from_str(&json).unwrap();
        match parsed {
            FilterConfig::LocalRateLimit(rate_limit_config) => {
                assert_eq!(rate_limit_config.stat_prefix, "ingress_http");
                assert!(rate_limit_config.token_bucket.is_some());
                assert_eq!(rate_limit_config.status_code, Some(429));
            }
            _ => panic!("Expected LocalRateLimit config"),
        }
    }

    #[test]
    fn test_local_rate_limit_filter_type() {
        use crate::xds::filters::http::local_rate_limit::{
            LocalRateLimitConfig, TokenBucketConfig,
        };

        let config = FilterConfig::LocalRateLimit(LocalRateLimitConfig {
            stat_prefix: "test".to_string(),
            token_bucket: Some(TokenBucketConfig {
                max_tokens: 10,
                tokens_per_fill: None,
                fill_interval_ms: 1000,
            }),
            status_code: None,
            filter_enabled: None,
            filter_enforced: None,
            per_downstream_connection: None,
            rate_limited_as_resource_exhausted: None,
            max_dynamic_descriptors: None,
            always_consume_default_token_bucket: None,
        });

        assert_eq!(config.filter_type(), FilterType::LocalRateLimit);
    }

    // CustomResponse filter tests

    #[test]
    fn test_custom_response_attaches_to_routes_and_listeners() {
        let ft = FilterType::CustomResponse;
        assert!(ft.can_attach_to(AttachmentPoint::Route));
        assert!(ft.can_attach_to(AttachmentPoint::Listener));
        assert!(!ft.can_attach_to(AttachmentPoint::Cluster));
        assert_eq!(
            ft.allowed_attachment_points(),
            vec![AttachmentPoint::Route, AttachmentPoint::Listener]
        );
    }

    #[test]
    fn test_custom_response_http_filter_name() {
        assert_eq!(
            FilterType::CustomResponse.http_filter_name(),
            "envoy.filters.http.custom_response"
        );
    }

    #[test]
    fn test_custom_response_from_str() {
        assert_eq!("custom_response".parse::<FilterType>().unwrap(), FilterType::CustomResponse);
    }

    #[test]
    fn test_custom_response_display() {
        assert_eq!(FilterType::CustomResponse.to_string(), "custom_response");
    }

    #[test]
    fn test_custom_response_filter_config_filter_type() {
        use crate::xds::filters::http::custom_response::{
            CustomResponseConfig, LocalResponsePolicy, ResponseMatcherRule, StatusCodeMatcher,
        };

        let config = FilterConfig::CustomResponse(CustomResponseConfig {
            matchers: vec![ResponseMatcherRule {
                status_code: StatusCodeMatcher::Exact { code: 429 },
                response: LocalResponsePolicy::json_error(429, "rate limited"),
            }],
            custom_response_matcher: None,
        });

        assert_eq!(config.filter_type(), FilterType::CustomResponse);
    }

    #[test]
    fn test_custom_response_filter_config_serialization() {
        use crate::xds::filters::http::custom_response::{
            CustomResponseConfig, LocalResponsePolicy, ResponseMatcherRule, StatusCodeMatcher,
        };

        let config = FilterConfig::CustomResponse(CustomResponseConfig {
            matchers: vec![ResponseMatcherRule {
                status_code: StatusCodeMatcher::Exact { code: 429 },
                response: LocalResponsePolicy::json_error(429, "rate limited"),
            }],
            custom_response_matcher: None,
        });

        let json = serde_json::to_string(&config).unwrap();
        // Should be tagged enum format
        assert!(json.contains(r#""type":"custom_response""#), "JSON: {}", json);
        assert!(json.contains(r#""config":"#), "JSON: {}", json);
        assert!(json.contains("429"), "JSON: {}", json);

        // Round-trip test
        let parsed: FilterConfig = serde_json::from_str(&json).unwrap();
        match parsed {
            FilterConfig::CustomResponse(cr_config) => {
                assert_eq!(cr_config.matchers.len(), 1);
                assert_eq!(
                    cr_config.matchers[0].status_code,
                    StatusCodeMatcher::Exact { code: 429 }
                );
            }
            _ => panic!("Expected CustomResponse config"),
        }
    }

    #[test]
    fn test_custom_response_filter_config_with_range_and_list() {
        use crate::xds::filters::http::custom_response::{
            CustomResponseConfig, LocalResponsePolicy, ResponseMatcherRule, StatusCodeMatcher,
        };

        let config = FilterConfig::CustomResponse(CustomResponseConfig {
            matchers: vec![
                ResponseMatcherRule {
                    status_code: StatusCodeMatcher::Range { min: 500, max: 599 },
                    response: LocalResponsePolicy::json_error(500, "server error"),
                },
                ResponseMatcherRule {
                    status_code: StatusCodeMatcher::List { codes: vec![400, 401, 403] },
                    response: LocalResponsePolicy::json_error(400, "client error"),
                },
            ],
            custom_response_matcher: None,
        });

        let json = serde_json::to_string(&config).unwrap();
        let parsed: FilterConfig = serde_json::from_str(&json).unwrap();

        match parsed {
            FilterConfig::CustomResponse(cr_config) => {
                assert_eq!(cr_config.matchers.len(), 2);
                assert!(matches!(
                    cr_config.matchers[0].status_code,
                    StatusCodeMatcher::Range { min: 500, max: 599 }
                ));
                assert!(matches!(
                    cr_config.matchers[1].status_code,
                    StatusCodeMatcher::List { .. }
                ));
            }
            _ => panic!("Expected CustomResponse config"),
        }
    }

    // FilterTypeMetadata tests

    #[test]
    fn test_metadata_returns_correct_values() {
        let metadata = FilterType::HeaderMutation.metadata();
        assert_eq!(metadata.filter_type, FilterType::HeaderMutation);
        assert_eq!(metadata.http_filter_name, "envoy.filters.http.header_mutation");
        assert_eq!(
            metadata.type_url,
            "type.googleapis.com/envoy.extensions.filters.http.header_mutation.v3.HeaderMutation"
        );
        assert!(metadata.per_route_type_url.is_some());
        assert_eq!(
            metadata.attachment_points,
            &[AttachmentPoint::Route, AttachmentPoint::Listener]
        );
        assert!(!metadata.requires_listener_config);
        assert_eq!(metadata.per_route_behavior, PerRouteBehavior::FullConfig);
        assert!(metadata.is_implemented);
    }

    #[test]
    fn test_metadata_consistency_with_existing_methods() {
        // Ensure metadata values match existing method implementations
        for filter_type in [
            FilterType::HeaderMutation,
            FilterType::JwtAuth,
            FilterType::Cors,
            FilterType::Compressor,
            FilterType::LocalRateLimit,
            FilterType::RateLimit,
            FilterType::ExtAuthz,
            FilterType::Rbac,
            FilterType::OAuth2,
            FilterType::CustomResponse,
            FilterType::Mcp,
        ] {
            let metadata = filter_type.metadata();

            // http_filter_name should match
            assert_eq!(
                metadata.http_filter_name,
                filter_type.http_filter_name(),
                "http_filter_name mismatch for {:?}",
                filter_type
            );

            // is_implemented should match
            assert_eq!(
                metadata.is_implemented,
                filter_type.is_fully_implemented(),
                "is_implemented mismatch for {:?}",
                filter_type
            );

            // requires_listener_config should match
            assert_eq!(
                metadata.requires_listener_config,
                filter_type.requires_listener_config(),
                "requires_listener_config mismatch for {:?}",
                filter_type
            );

            // attachment_points should match (compare as vec)
            assert_eq!(
                metadata.attachment_points.to_vec(),
                filter_type.allowed_attachment_points(),
                "attachment_points mismatch for {:?}",
                filter_type
            );
        }
    }

    #[test]
    fn test_per_route_behavior_values() {
        assert_eq!(FilterType::HeaderMutation.per_route_behavior(), PerRouteBehavior::FullConfig);
        assert_eq!(FilterType::JwtAuth.per_route_behavior(), PerRouteBehavior::ReferenceOnly);
        assert_eq!(FilterType::LocalRateLimit.per_route_behavior(), PerRouteBehavior::FullConfig);
        assert_eq!(FilterType::CustomResponse.per_route_behavior(), PerRouteBehavior::FullConfig);
        assert_eq!(FilterType::Mcp.per_route_behavior(), PerRouteBehavior::DisableOnly);
    }

    #[test]
    fn test_from_http_filter_name() {
        assert_eq!(
            FilterType::from_http_filter_name("envoy.filters.http.header_mutation"),
            Some(FilterType::HeaderMutation)
        );
        assert_eq!(
            FilterType::from_http_filter_name("envoy.filters.http.jwt_authn"),
            Some(FilterType::JwtAuth)
        );
        assert_eq!(
            FilterType::from_http_filter_name("envoy.filters.http.custom_response"),
            Some(FilterType::CustomResponse)
        );
        assert_eq!(
            FilterType::from_http_filter_name("envoy.filters.http.mcp"),
            Some(FilterType::Mcp)
        );
        assert_eq!(FilterType::from_http_filter_name("unknown.filter"), None);
    }

    #[test]
    fn test_all_filter_types_have_metadata() {
        // Ensure all filter types return valid metadata (no panic)
        let all_types = [
            FilterType::HeaderMutation,
            FilterType::JwtAuth,
            FilterType::Cors,
            FilterType::Compressor,
            FilterType::LocalRateLimit,
            FilterType::RateLimit,
            FilterType::ExtAuthz,
            FilterType::Rbac,
            FilterType::OAuth2,
            FilterType::CustomResponse,
            FilterType::Mcp,
        ];

        for ft in all_types {
            let m = ft.metadata();
            assert_eq!(m.filter_type, ft, "Metadata filter_type should match for {:?}", ft);
            assert!(!m.http_filter_name.is_empty(), "http_filter_name should not be empty");
            assert!(!m.type_url.is_empty(), "type_url should not be empty");
            assert!(!m.description.is_empty(), "description should not be empty");
        }
    }

    #[test]
    fn test_oauth2_serialization_consistency() {
        // Ensure OAuth2 uses "oauth2" not "o_auth2" for both FilterType and FilterConfig
        let ft = FilterType::OAuth2;

        // FilterType serialization
        let json = serde_json::to_string(&ft).unwrap();
        assert_eq!(json, r#""oauth2""#, "FilterType::OAuth2 should serialize as oauth2");

        // FilterType deserialization
        let parsed: FilterType = serde_json::from_str(r#""oauth2""#).unwrap();
        assert_eq!(parsed, FilterType::OAuth2);

        // Ensure "o_auth2" does NOT deserialize (it's invalid)
        let invalid: Result<FilterType, _> = serde_json::from_str(r#""o_auth2""#);
        assert!(invalid.is_err(), "o_auth2 should not be accepted");

        // Display and FromStr consistency
        assert_eq!(ft.to_string(), "oauth2");
        assert_eq!("oauth2".parse::<FilterType>().unwrap(), FilterType::OAuth2);
    }

    #[test]
    fn test_custom_filter_deserialization() {
        // Test that unknown filter types are parsed as Custom
        let json = r#"{
            "type": "wasm",
            "config": {
                "name": "add_header",
                "vm_config": {
                    "runtime": "envoy.wasm.runtime.v8",
                    "code": {
                        "local": {
                            "filename": "/path/to/filter.wasm"
                        }
                    }
                }
            }
        }"#;

        let result: Result<FilterConfig, _> = serde_json::from_str(json);
        assert!(result.is_ok(), "Should parse custom filter: {:?}", result.err());

        let config = result.unwrap();
        assert!(config.is_custom(), "Should be a custom filter");
        assert_eq!(config.filter_type_str(), "wasm");

        if let FilterConfig::Custom(custom) = config {
            assert_eq!(custom.filter_type, "wasm");
            assert!(custom.config.get("name").is_some());
            assert!(custom.config.get("vm_config").is_some());
        } else {
            panic!("Expected Custom filter config");
        }
    }

    #[test]
    fn test_custom_filter_methods() {
        let custom = FilterConfig::Custom(CustomFilterConfig {
            filter_type: "lua".to_string(),
            config: serde_json::json!({
                "inline_code": "function envoy_on_request(handle) end"
            }),
        });

        assert!(custom.is_custom());
        assert_eq!(custom.filter_type_str(), "lua");
        assert!(custom.try_filter_type().is_none());
        assert!(custom.custom_config().is_some());
    }

    #[test]
    fn test_custom_filter_serialization_roundtrip() {
        // Test that custom filters can be serialized and deserialized correctly
        let original = FilterConfig::Custom(CustomFilterConfig {
            filter_type: "wasm".to_string(),
            config: serde_json::json!({
                "name": "add_header",
                "vm_config": {
                    "runtime": "envoy.wasm.runtime.v8",
                    "code": {
                        "local": {
                            "filename": "/path/to/filter.wasm"
                        }
                    }
                }
            }),
        });

        // Serialize to JSON
        let json = serde_json::to_string(&original).expect("Should serialize custom filter");

        // Verify the JSON structure uses the dynamic type name, not "custom"
        assert!(json.contains(r#""type":"wasm""#), "JSON should have type=wasm: {}", json);
        assert!(json.contains(r#""config":"#), "JSON should have config field: {}", json);
        assert!(!json.contains(r#""type":"custom""#), "JSON should NOT have type=custom: {}", json);

        // Deserialize back
        let parsed: FilterConfig = serde_json::from_str(&json).expect("Should deserialize");

        // Verify round-trip
        assert!(parsed.is_custom());
        assert_eq!(parsed.filter_type_str(), "wasm");

        if let FilterConfig::Custom(custom) = parsed {
            assert_eq!(custom.filter_type, "wasm");
            assert_eq!(custom.config["name"], "add_header");
            assert_eq!(custom.config["vm_config"]["runtime"], "envoy.wasm.runtime.v8");
        } else {
            panic!("Expected Custom filter after round-trip");
        }
    }

    #[test]
    fn test_custom_filter_lua_serialization_roundtrip() {
        // Test another custom filter type to ensure it's not hardcoded to "wasm"
        let original = FilterConfig::Custom(CustomFilterConfig {
            filter_type: "lua".to_string(),
            config: serde_json::json!({
                "inline_code": "function envoy_on_request(handle) handle:headers():add('x-lua', 'processed') end"
            }),
        });

        let json = serde_json::to_string(&original).expect("Should serialize lua filter");

        // Verify the JSON uses "lua" not "custom" or "wasm"
        assert!(json.contains(r#""type":"lua""#), "JSON should have type=lua: {}", json);

        let parsed: FilterConfig = serde_json::from_str(&json).expect("Should deserialize");
        assert_eq!(parsed.filter_type_str(), "lua");
    }
}
