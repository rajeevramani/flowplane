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
    RateLimit,
    ExtAuthz,
}

impl FilterType {
    /// Returns the valid attachment points for this filter type.
    ///
    /// Filter types have different scopes:
    /// - HeaderMutation, Cors: Route-level only (L7 HTTP route filters)
    /// - JwtAuth, RateLimit, ExtAuthz: Can apply at both route and listener levels
    pub fn allowed_attachment_points(&self) -> Vec<AttachmentPoint> {
        match self {
            FilterType::HeaderMutation => vec![AttachmentPoint::Route],
            FilterType::Cors => vec![AttachmentPoint::Route],
            FilterType::JwtAuth => vec![AttachmentPoint::Route, AttachmentPoint::Listener],
            FilterType::RateLimit => vec![AttachmentPoint::Route, AttachmentPoint::Listener],
            FilterType::ExtAuthz => vec![AttachmentPoint::Route, AttachmentPoint::Listener],
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
}

impl fmt::Display for FilterType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FilterType::HeaderMutation => write!(f, "header_mutation"),
            FilterType::JwtAuth => write!(f, "jwt_auth"),
            FilterType::Cors => write!(f, "cors"),
            FilterType::RateLimit => write!(f, "rate_limit"),
            FilterType::ExtAuthz => write!(f, "ext_authz"),
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
            "rate_limit" => Ok(FilterType::RateLimit),
            "ext_authz" => Ok(FilterType::ExtAuthz),
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", content = "config", rename_all = "snake_case")]
pub enum FilterConfig {
    HeaderMutation(HeaderMutationFilterConfig),
    // Future filter types will be added here:
    // JwtAuth(JwtAuthConfig),
    // Cors(CorsConfig),
    // RateLimit(RateLimitConfig),
}

impl FilterConfig {
    pub fn filter_type(&self) -> FilterType {
        match self {
            FilterConfig::HeaderMutation(_) => FilterType::HeaderMutation,
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
        assert_eq!("rate_limit".parse::<FilterType>().unwrap(), FilterType::RateLimit);
        assert_eq!("ext_authz".parse::<FilterType>().unwrap(), FilterType::ExtAuthz);

        // Unknown type should error
        assert!("unknown".parse::<FilterType>().is_err());
    }
}
