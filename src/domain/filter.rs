use crate::xds::filters::http::custom_response::CustomResponseConfig;
use crate::xds::filters::http::jwt_auth::JwtAuthenticationConfig;
use crate::xds::filters::http::local_rate_limit::LocalRateLimitConfig;
use crate::xds::filters::http::mcp::McpFilterConfig;
use serde::{Deserialize, Serialize};
use std::fmt;
use utoipa::ToSchema;

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
    /// Local (in-memory) rate limiting
    LocalRateLimit,
    /// External/distributed rate limiting (requires gRPC service)
    RateLimit,
    ExtAuthz,
    /// Custom response filter for modifying responses based on status codes
    CustomResponse,
    /// Model Context Protocol (MCP) filter for AI/LLM gateway traffic
    /// Inspects and validates JSON-RPC 2.0 and SSE stream traffic
    Mcp,
}

impl FilterType {
    /// Returns the valid attachment points for this filter type.
    ///
    /// Filter types have different scopes:
    /// - HeaderMutation, Cors: Route-level only (L7 HTTP route filters)
    /// - JwtAuth, RateLimit, ExtAuthz, CustomResponse: Can apply at both route and listener levels
    pub fn allowed_attachment_points(&self) -> Vec<AttachmentPoint> {
        match self {
            FilterType::HeaderMutation => vec![AttachmentPoint::Route],
            FilterType::Cors => vec![AttachmentPoint::Route],
            FilterType::JwtAuth => vec![AttachmentPoint::Route, AttachmentPoint::Listener],
            FilterType::LocalRateLimit => vec![AttachmentPoint::Route, AttachmentPoint::Listener],
            FilterType::RateLimit => vec![AttachmentPoint::Route, AttachmentPoint::Listener],
            FilterType::ExtAuthz => vec![AttachmentPoint::Route, AttachmentPoint::Listener],
            FilterType::CustomResponse => vec![AttachmentPoint::Route, AttachmentPoint::Listener],
            FilterType::Mcp => vec![AttachmentPoint::Route, AttachmentPoint::Listener],
        }
    }

    /// Checks if this filter type can attach to the given attachment point.
    pub fn can_attach_to(&self, point: AttachmentPoint) -> bool {
        self.allowed_attachment_points().contains(&point)
    }

    /// Returns a human-readable description of allowed attachment points.
    pub fn allowed_attachment_points_display(&self) -> String {
        let points = self.allowed_attachment_points();
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
        match self {
            FilterType::HeaderMutation => "envoy.filters.http.header_mutation",
            FilterType::JwtAuth => "envoy.filters.http.jwt_authn",
            FilterType::Cors => "envoy.filters.http.cors",
            FilterType::LocalRateLimit => "envoy.filters.http.local_ratelimit",
            FilterType::RateLimit => "envoy.filters.http.ratelimit",
            FilterType::ExtAuthz => "envoy.filters.http.ext_authz",
            FilterType::CustomResponse => "envoy.filters.http.custom_response",
            FilterType::Mcp => "envoy.filters.http.mcp",
        }
    }

    /// Returns true if this filter type has full implementation support.
    ///
    /// Used for API validation to reject unsupported filter creation.
    /// Filter types that are defined but not yet fully implemented will return false.
    pub fn is_fully_implemented(&self) -> bool {
        matches!(
            self,
            FilterType::HeaderMutation
                | FilterType::JwtAuth
                | FilterType::LocalRateLimit
                | FilterType::CustomResponse
                | FilterType::Mcp
        )
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
        matches!(
            self,
            FilterType::JwtAuth
                | FilterType::LocalRateLimit
                | FilterType::RateLimit
                | FilterType::ExtAuthz
                | FilterType::CustomResponse
                | FilterType::Mcp
        )
    }
}

impl fmt::Display for FilterType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FilterType::HeaderMutation => write!(f, "header_mutation"),
            FilterType::JwtAuth => write!(f, "jwt_auth"),
            FilterType::Cors => write!(f, "cors"),
            FilterType::LocalRateLimit => write!(f, "local_rate_limit"),
            FilterType::RateLimit => write!(f, "rate_limit"),
            FilterType::ExtAuthz => write!(f, "ext_authz"),
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
            "local_rate_limit" => Ok(FilterType::LocalRateLimit),
            "rate_limit" => Ok(FilterType::RateLimit),
            "ext_authz" => Ok(FilterType::ExtAuthz),
            "custom_response" => Ok(FilterType::CustomResponse),
            "mcp" => Ok(FilterType::Mcp),
            _ => Err(format!("Unknown filter type: {}", s)),
        }
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

/// Envelope for all filter configurations (extensible for future filter types)
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", content = "config", rename_all = "snake_case")]
pub enum FilterConfig {
    HeaderMutation(HeaderMutationFilterConfig),
    JwtAuth(JwtAuthenticationConfig),
    LocalRateLimit(LocalRateLimitConfig),
    CustomResponse(CustomResponseConfig),
    /// Model Context Protocol (MCP) filter configuration
    Mcp(McpFilterConfig),
    // Future filter types will be added here:
    // Cors(CorsConfig),
}

impl FilterConfig {
    pub fn filter_type(&self) -> FilterType {
        match self {
            FilterConfig::HeaderMutation(_) => FilterType::HeaderMutation,
            FilterConfig::JwtAuth(_) => FilterType::JwtAuth,
            FilterConfig::LocalRateLimit(_) => FilterType::LocalRateLimit,
            FilterConfig::CustomResponse(_) => FilterType::CustomResponse,
            FilterConfig::Mcp(_) => FilterType::Mcp,
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
        assert_eq!(req.filter_type, FilterType::HeaderMutation);
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
    fn test_header_mutation_only_attaches_to_routes() {
        let ft = FilterType::HeaderMutation;
        assert!(ft.can_attach_to(AttachmentPoint::Route));
        assert!(!ft.can_attach_to(AttachmentPoint::Listener));
        assert!(!ft.can_attach_to(AttachmentPoint::Cluster));
        assert_eq!(ft.allowed_attachment_points(), vec![AttachmentPoint::Route]);
    }

    #[test]
    fn test_cors_only_attaches_to_routes() {
        let ft = FilterType::Cors;
        assert!(ft.can_attach_to(AttachmentPoint::Route));
        assert!(!ft.can_attach_to(AttachmentPoint::Listener));
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
        assert_eq!(FilterType::HeaderMutation.allowed_attachment_points_display(), "route only");
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

        // Not yet implemented filters
        assert!(!FilterType::Cors.is_fully_implemented());
        assert!(!FilterType::RateLimit.is_fully_implemented());
        assert!(!FilterType::ExtAuthz.is_fully_implemented());
    }

    #[test]
    fn test_requires_listener_config() {
        // Filters that require listener-level configuration (cannot be empty placeholders)
        assert!(FilterType::JwtAuth.requires_listener_config());
        assert!(FilterType::LocalRateLimit.requires_listener_config());
        assert!(FilterType::RateLimit.requires_listener_config());
        assert!(FilterType::ExtAuthz.requires_listener_config());
        assert!(FilterType::CustomResponse.requires_listener_config());

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
}
