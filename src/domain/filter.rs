use serde::{Deserialize, Serialize};
use std::fmt;
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FilterType {
    HeaderMutation,
    JwtAuth,
    Cors,
    RateLimit,
    ExtAuthz,
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
}
