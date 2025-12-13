//! OAuth2 HTTP filter configuration
//!
//! This module provides configuration types for the Envoy OAuth2 filter,
//! which enables OAuth2 authentication flows for HTTP requests.

use crate::xds::filters::{any_from_message, invalid_config};
use envoy_types::pb::envoy::config::core::v3::{
    config_source::ConfigSourceSpecifier, AggregatedConfigSource, ConfigSource, HttpUri,
};
use envoy_types::pb::envoy::extensions::filters::http::oauth2::v3::{
    o_auth2_config::AuthType, o_auth2_credentials::CookieNames,
    o_auth2_credentials::TokenFormation, OAuth2, OAuth2Config as OAuth2ConfigProto,
    OAuth2Credentials,
};
use envoy_types::pb::envoy::extensions::transport_sockets::tls::v3::SdsSecretConfig;
use envoy_types::pb::envoy::r#type::matcher::v3::PathMatcher;
use envoy_types::pb::google::protobuf::Any as EnvoyAny;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Build an SDS secret config that uses ADS for secret discovery
fn build_sds_secret_config(name: &str) -> SdsSecretConfig {
    SdsSecretConfig {
        name: name.to_string(),
        sds_config: Some(ConfigSource {
            config_source_specifier: Some(ConfigSourceSpecifier::Ads(
                AggregatedConfigSource::default(),
            )),
            ..Default::default()
        }),
    }
}

/// Type URLs for OAuth2 filter configuration
pub const OAUTH2_TYPE_URL: &str =
    "type.googleapis.com/envoy.extensions.filters.http.oauth2.v3.OAuth2";

/// OAuth2 authentication type
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum OAuth2AuthType {
    /// URL-encoded body authentication (default)
    #[default]
    UrlEncodedBody,
    /// Basic authentication
    BasicAuth,
}

impl OAuth2AuthType {
    fn to_proto(self) -> i32 {
        match self {
            Self::UrlEncodedBody => AuthType::UrlEncodedBody as i32,
            Self::BasicAuth => AuthType::BasicAuth as i32,
        }
    }
}

/// Token secret configuration
/// Secrets are always delivered via SDS (Secret Discovery Service)
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TokenSecretConfig {
    /// Name of the SDS secret
    pub name: String,
}

impl Default for TokenSecretConfig {
    fn default() -> Self {
        Self { name: "oauth2-token-secret".to_string() }
    }
}

/// OAuth2 credentials configuration
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct OAuth2CredentialsConfig {
    /// OAuth2 client ID
    pub client_id: String,
    /// Token secret configuration
    #[serde(default)]
    pub token_secret: Option<TokenSecretConfig>,
    /// Cookie domain for OAuth cookies
    #[serde(default)]
    pub cookie_domain: Option<String>,
    /// Custom cookie names
    #[serde(default)]
    pub cookie_names: Option<OAuth2CookieNames>,
}

/// Custom OAuth2 cookie names
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Default)]
pub struct OAuth2CookieNames {
    #[serde(default)]
    pub bearer_token: Option<String>,
    #[serde(default)]
    pub oauth_hmac: Option<String>,
    #[serde(default)]
    pub oauth_expires: Option<String>,
    #[serde(default)]
    pub id_token: Option<String>,
    #[serde(default)]
    pub refresh_token: Option<String>,
}

/// Token endpoint configuration
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TokenEndpointConfig {
    /// URI for the token endpoint
    pub uri: String,
    /// Cluster name for the token endpoint
    pub cluster: String,
    /// Timeout in milliseconds
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
}

fn default_timeout_ms() -> u64 {
    5000
}

/// OAuth2 filter configuration
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct OAuth2Config {
    /// Token endpoint configuration
    pub token_endpoint: TokenEndpointConfig,
    /// Authorization endpoint URL
    pub authorization_endpoint: String,
    /// OAuth2 credentials
    pub credentials: OAuth2CredentialsConfig,
    /// Redirect URI (the callback URL)
    pub redirect_uri: String,
    /// Path that handles the OAuth2 callback
    #[serde(default = "default_redirect_path")]
    pub redirect_path: String,
    /// Path for signing out (clears OAuth cookies)
    #[serde(default)]
    pub signout_path: Option<String>,
    /// OAuth2 scopes to request
    #[serde(default = "default_auth_scopes")]
    pub auth_scopes: Vec<String>,
    /// Authentication type
    #[serde(default)]
    pub auth_type: OAuth2AuthType,
    /// Forward the bearer token to upstream
    #[serde(default = "default_forward_bearer_token")]
    pub forward_bearer_token: bool,
    /// Preserve existing authorization header
    #[serde(default)]
    pub preserve_authorization_header: bool,
    /// Use refresh tokens for automatic renewal
    #[serde(default)]
    pub use_refresh_token: bool,
    /// Default token expiration in seconds (if not provided by server)
    #[serde(default)]
    pub default_expires_in_seconds: Option<u64>,
    /// Stat prefix for metrics
    #[serde(default)]
    pub stat_prefix: Option<String>,
}

fn default_redirect_path() -> String {
    "/oauth2/callback".to_string()
}

fn default_auth_scopes() -> Vec<String> {
    vec!["openid".to_string(), "profile".to_string(), "email".to_string()]
}

fn default_forward_bearer_token() -> bool {
    true
}

impl OAuth2Config {
    /// Validate configuration
    pub fn validate(&self) -> Result<(), crate::Error> {
        if self.credentials.client_id.is_empty() {
            return Err(invalid_config("OAuth2 client_id is required"));
        }
        if self.token_endpoint.uri.is_empty() {
            return Err(invalid_config("OAuth2 token_endpoint.uri is required"));
        }
        if self.token_endpoint.cluster.is_empty() {
            return Err(invalid_config("OAuth2 token_endpoint.cluster is required"));
        }
        if self.authorization_endpoint.is_empty() {
            return Err(invalid_config("OAuth2 authorization_endpoint is required"));
        }
        if self.redirect_uri.is_empty() {
            return Err(invalid_config("OAuth2 redirect_uri is required"));
        }
        Ok(())
    }

    /// Convert to Envoy Any protobuf
    pub fn to_any(&self) -> Result<EnvoyAny, crate::Error> {
        self.validate()?;

        // Build credentials with proper SDS configuration for secret discovery via ADS
        let credentials = OAuth2Credentials {
            client_id: self.credentials.client_id.clone(),
            token_secret: self
                .credentials
                .token_secret
                .as_ref()
                .map(|ts| build_sds_secret_config(&ts.name)),
            cookie_names: self.credentials.cookie_names.as_ref().map(|cn| CookieNames {
                bearer_token: cn.bearer_token.clone().unwrap_or_default(),
                oauth_hmac: cn.oauth_hmac.clone().unwrap_or_default(),
                oauth_expires: cn.oauth_expires.clone().unwrap_or_default(),
                id_token: cn.id_token.clone().unwrap_or_default(),
                refresh_token: cn.refresh_token.clone().unwrap_or_default(),
                code_verifier: String::new(),
                oauth_nonce: String::new(),
            }),
            cookie_domain: self.credentials.cookie_domain.clone().unwrap_or_default(),
            token_formation: Some(TokenFormation::HmacSecret(build_sds_secret_config(
                "hmac-secret",
            ))),
        };

        // Build token endpoint HTTP URI
        let token_endpoint = HttpUri {
            uri: self.token_endpoint.uri.clone(),
            http_upstream_type: Some(
                envoy_types::pb::envoy::config::core::v3::http_uri::HttpUpstreamType::Cluster(
                    self.token_endpoint.cluster.clone(),
                ),
            ),
            timeout: Some(envoy_types::pb::google::protobuf::Duration {
                seconds: (self.token_endpoint.timeout_ms / 1000) as i64,
                nanos: ((self.token_endpoint.timeout_ms % 1000) * 1_000_000) as i32,
            }),
        };

        // Build redirect path matcher
        let redirect_path_matcher = PathMatcher {
            rule: Some(
                envoy_types::pb::envoy::r#type::matcher::v3::path_matcher::Rule::Path(
                    envoy_types::pb::envoy::r#type::matcher::v3::StringMatcher {
                        match_pattern: Some(
                            envoy_types::pb::envoy::r#type::matcher::v3::string_matcher::MatchPattern::Exact(
                                self.redirect_path.clone(),
                            ),
                        ),
                        ignore_case: false,
                    },
                ),
            ),
        };

        let config = OAuth2ConfigProto {
            token_endpoint: Some(token_endpoint),
            authorization_endpoint: self.authorization_endpoint.clone(),
            credentials: Some(credentials),
            redirect_uri: self.redirect_uri.clone(),
            redirect_path_matcher: Some(redirect_path_matcher),
            signout_path: self
                .signout_path
                .as_ref()
                .map(|p| PathMatcher {
                    rule: Some(
                        envoy_types::pb::envoy::r#type::matcher::v3::path_matcher::Rule::Path(
                            envoy_types::pb::envoy::r#type::matcher::v3::StringMatcher {
                                match_pattern: Some(
                                    envoy_types::pb::envoy::r#type::matcher::v3::string_matcher::MatchPattern::Exact(
                                        p.clone(),
                                    ),
                                ),
                                ignore_case: false,
                            },
                        ),
                    ),
                }),
            auth_scopes: self.auth_scopes.clone(),
            auth_type: self.auth_type.to_proto(),
            forward_bearer_token: self.forward_bearer_token,
            preserve_authorization_header: self.preserve_authorization_header,
            use_refresh_token: Some(envoy_types::pb::google::protobuf::BoolValue {
                value: self.use_refresh_token,
            }),
            default_expires_in: self.default_expires_in_seconds.map(|s| {
                envoy_types::pb::google::protobuf::Duration {
                    seconds: s as i64,
                    nanos: 0,
                }
            }),
            pass_through_matcher: Vec::new(),
            resources: Vec::new(),
            default_refresh_token_expires_in: None,
            deny_redirect_matcher: Vec::new(),
            retry_policy: None,
            cookie_configs: None,
            end_session_endpoint: String::new(),
            disable_id_token_set_cookie: false,
            disable_access_token_set_cookie: false,
            disable_refresh_token_set_cookie: false,
            csrf_token_expires_in: None,
            code_verifier_token_expires_in: None,
            stat_prefix: self.stat_prefix.clone().unwrap_or_default(),
            disable_token_encryption: false,
        };

        let proto = OAuth2 { config: Some(config) };

        Ok(any_from_message(OAUTH2_TYPE_URL, &proto))
    }
}

/// Per-route OAuth2 configuration
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Default)]
pub struct OAuth2PerRouteConfig {
    /// Disable OAuth2 for this route
    #[serde(default)]
    pub disabled: bool,
}

impl OAuth2PerRouteConfig {
    /// Convert to Envoy Any protobuf
    pub fn to_any(&self) -> Result<EnvoyAny, crate::Error> {
        let proto = if self.disabled {
            // Empty config disables OAuth2 for this route
            OAuth2 { config: None }
        } else {
            OAuth2::default()
        };

        Ok(any_from_message(OAUTH2_TYPE_URL, &proto))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_oauth2_config_validation() {
        // Empty config should fail
        let config = OAuth2Config {
            token_endpoint: TokenEndpointConfig {
                uri: String::new(),
                cluster: String::new(),
                timeout_ms: 5000,
            },
            authorization_endpoint: String::new(),
            credentials: OAuth2CredentialsConfig {
                client_id: String::new(),
                token_secret: None,
                cookie_domain: None,
                cookie_names: None,
            },
            redirect_uri: String::new(),
            redirect_path: default_redirect_path(),
            signout_path: None,
            auth_scopes: default_auth_scopes(),
            auth_type: OAuth2AuthType::default(),
            forward_bearer_token: true,
            preserve_authorization_header: false,
            use_refresh_token: false,
            default_expires_in_seconds: None,
            stat_prefix: None,
        };

        assert!(config.validate().is_err());
    }

    #[test]
    fn test_valid_oauth2_config() {
        let config = OAuth2Config {
            token_endpoint: TokenEndpointConfig {
                uri: "https://auth.example.com/oauth/token".to_string(),
                cluster: "auth-cluster".to_string(),
                timeout_ms: 5000,
            },
            authorization_endpoint: "https://auth.example.com/oauth/authorize".to_string(),
            credentials: OAuth2CredentialsConfig {
                client_id: "my-client-id".to_string(),
                token_secret: Some(TokenSecretConfig { name: "oauth-secret".to_string() }),
                cookie_domain: Some("example.com".to_string()),
                cookie_names: None,
            },
            redirect_uri: "https://app.example.com/oauth2/callback".to_string(),
            redirect_path: "/oauth2/callback".to_string(),
            signout_path: Some("/logout".to_string()),
            auth_scopes: vec!["openid".to_string(), "profile".to_string()],
            auth_type: OAuth2AuthType::UrlEncodedBody,
            forward_bearer_token: true,
            preserve_authorization_header: false,
            use_refresh_token: true,
            default_expires_in_seconds: Some(3600),
            stat_prefix: Some("oauth2".to_string()),
        };

        assert!(config.validate().is_ok());
        let any = config.to_any().expect("to_any should succeed");
        assert_eq!(any.type_url, OAUTH2_TYPE_URL);
    }

    #[test]
    fn test_per_route_disabled() {
        let config = OAuth2PerRouteConfig { disabled: true };
        let any = config.to_any().expect("to_any");
        assert_eq!(any.type_url, OAUTH2_TYPE_URL);
    }
}
