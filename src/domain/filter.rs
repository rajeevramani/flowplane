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
