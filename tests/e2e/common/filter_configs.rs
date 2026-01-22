//! Type-safe Filter Configuration Builders
//!
//! Provides type-safe builders for each filter type to ensure consistency
//! across tests. Each builder produces a serde_json::Value that can be used
//! with the API client's create_filter method.
//!
//! Example usage:
//! ```rust,ignore
//! let config = filter_configs::rate_limit()
//!     .max_tokens(10)
//!     .fill_interval_ms(60000)
//!     .status_code(429)
//!     .build();
//! ```

use serde_json::{json, Value};

/// Builder for local_rate_limit filter configuration
#[derive(Default)]
pub struct RateLimitBuilder {
    stat_prefix: String,
    max_tokens: u32,
    tokens_per_fill: u32,
    fill_interval_ms: u64,
    status_code: u16,
    filter_enabled_percent: u8,
    filter_enforced_percent: u8,
}

impl RateLimitBuilder {
    pub fn new() -> Self {
        Self {
            stat_prefix: "rate_limit".to_string(),
            max_tokens: 10,
            tokens_per_fill: 10,
            fill_interval_ms: 60000,
            status_code: 429,
            filter_enabled_percent: 100,
            filter_enforced_percent: 100,
        }
    }

    pub fn stat_prefix(mut self, prefix: &str) -> Self {
        self.stat_prefix = prefix.to_string();
        self
    }

    pub fn max_tokens(mut self, tokens: u32) -> Self {
        self.max_tokens = tokens;
        self
    }

    pub fn tokens_per_fill(mut self, tokens: u32) -> Self {
        self.tokens_per_fill = tokens;
        self
    }

    pub fn fill_interval_ms(mut self, ms: u64) -> Self {
        self.fill_interval_ms = ms;
        self
    }

    pub fn status_code(mut self, code: u16) -> Self {
        self.status_code = code;
        self
    }

    pub fn filter_enabled_percent(mut self, percent: u8) -> Self {
        self.filter_enabled_percent = percent;
        self
    }

    pub fn filter_enforced_percent(mut self, percent: u8) -> Self {
        self.filter_enforced_percent = percent;
        self
    }

    pub fn build(self) -> Value {
        json!({
            "type": "local_rate_limit",
            "config": {
                "stat_prefix": self.stat_prefix,
                "token_bucket": {
                    "max_tokens": self.max_tokens,
                    "tokens_per_fill": self.tokens_per_fill,
                    "fill_interval_ms": self.fill_interval_ms
                },
                "status_code": self.status_code,
                "filter_enabled": {
                    "numerator": self.filter_enabled_percent,
                    "denominator": "hundred"
                },
                "filter_enforced": {
                    "numerator": self.filter_enforced_percent,
                    "denominator": "hundred"
                }
            }
        })
    }
}

/// Builder for header_mutation filter configuration
#[derive(Default)]
pub struct HeaderMutationBuilder {
    response_headers: Vec<(String, String, String)>, // (key, value, action)
    request_headers: Vec<(String, String, String)>,
}

impl HeaderMutationBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_response_header(mut self, key: &str, value: &str) -> Self {
        self.response_headers.push((
            key.to_string(),
            value.to_string(),
            "OVERWRITE_IF_EXISTS_OR_ADD".to_string(),
        ));
        self
    }

    pub fn add_response_header_with_action(mut self, key: &str, value: &str, action: &str) -> Self {
        self.response_headers.push((key.to_string(), value.to_string(), action.to_string()));
        self
    }

    pub fn add_request_header(mut self, key: &str, value: &str) -> Self {
        self.request_headers.push((
            key.to_string(),
            value.to_string(),
            "OVERWRITE_IF_EXISTS_OR_ADD".to_string(),
        ));
        self
    }

    pub fn build(self) -> Value {
        let mut config = json!({});

        if !self.response_headers.is_empty() {
            let headers: Vec<Value> = self
                .response_headers
                .into_iter()
                .map(|(key, value, action)| {
                    json!({
                        "header": {"key": key, "value": value},
                        "append_action": action
                    })
                })
                .collect();
            config["response_headers_to_add"] = json!(headers);
        }

        if !self.request_headers.is_empty() {
            let headers: Vec<Value> = self
                .request_headers
                .into_iter()
                .map(|(key, value, action)| {
                    json!({
                        "header": {"key": key, "value": value},
                        "append_action": action
                    })
                })
                .collect();
            config["request_headers_to_add"] = json!(headers);
        }

        config
    }
}

/// Builder for jwt_auth filter configuration
pub struct JwtAuthBuilder {
    providers: Vec<JwtProvider>,
    rules: Vec<JwtRule>,
    bypass_cors_preflight: bool,
}

pub struct JwtProvider {
    name: String,
    issuer: String,
    audiences: Vec<String>,
    jwks_uri: String,
    jwks_cluster: String,
    forward: bool,
    claim_to_headers: Vec<(String, String)>,
}

pub struct JwtRule {
    path_prefix: String,
    requires: JwtRequirement,
}

pub enum JwtRequirement {
    ProviderName(String),
    AllowMissing,
    AllowMissingOrFailed,
}

impl JwtAuthBuilder {
    pub fn new() -> Self {
        Self { providers: Vec::new(), rules: Vec::new(), bypass_cors_preflight: true }
    }

    pub fn add_provider(
        mut self,
        name: &str,
        issuer: &str,
        audiences: Vec<&str>,
        jwks_uri: &str,
        jwks_cluster: &str,
    ) -> Self {
        self.providers.push(JwtProvider {
            name: name.to_string(),
            issuer: issuer.to_string(),
            audiences: audiences.into_iter().map(String::from).collect(),
            jwks_uri: jwks_uri.to_string(),
            jwks_cluster: jwks_cluster.to_string(),
            forward: true,
            claim_to_headers: Vec::new(),
        });
        self
    }

    pub fn add_rule(mut self, path_prefix: &str, requires: JwtRequirement) -> Self {
        self.rules.push(JwtRule { path_prefix: path_prefix.to_string(), requires });
        self
    }

    pub fn bypass_cors_preflight(mut self, bypass: bool) -> Self {
        self.bypass_cors_preflight = bypass;
        self
    }

    pub fn build(self) -> Value {
        let providers: serde_json::Map<String, Value> = self
            .providers
            .into_iter()
            .map(|p| {
                let mut provider = json!({
                    "issuer": p.issuer,
                    "audiences": p.audiences,
                    "jwks": {
                        "type": "remote",
                        "http_uri": {
                            "uri": p.jwks_uri,
                            "cluster": p.jwks_cluster,
                            "timeout_seconds": 5
                        }
                    },
                    "forward": p.forward
                });
                if !p.claim_to_headers.is_empty() {
                    provider["claim_to_headers"] = json!(p
                        .claim_to_headers
                        .into_iter()
                        .map(|(header, claim)| json!({"header_name": header, "claim_name": claim}))
                        .collect::<Vec<_>>());
                }
                (p.name, provider)
            })
            .collect();

        let rules: Vec<Value> = self
            .rules
            .into_iter()
            .map(|r| {
                let requires = match r.requires {
                    JwtRequirement::ProviderName(name) => {
                        json!({"type": "provider_name", "provider_name": name})
                    }
                    JwtRequirement::AllowMissing => json!({"type": "allow_missing"}),
                    JwtRequirement::AllowMissingOrFailed => {
                        json!({"type": "allow_missing_or_failed"})
                    }
                };
                json!({
                    "match": {"path": {"type": "prefix", "value": r.path_prefix}},
                    "requires": requires
                })
            })
            .collect();

        json!({
            "providers": providers,
            "rules": rules,
            "bypass_cors_preflight": self.bypass_cors_preflight
        })
    }
}

impl Default for JwtAuthBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Builder for cors filter configuration
#[derive(Default)]
pub struct CorsBuilder {
    allow_origins: Vec<(String, String)>, // (type, value)
    allow_methods: Vec<String>,
    allow_headers: Vec<String>,
    expose_headers: Vec<String>,
    max_age: Option<u64>,
    allow_credentials: bool,
}

impl CorsBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn allow_origin_exact(mut self, origin: &str) -> Self {
        self.allow_origins.push(("exact".to_string(), origin.to_string()));
        self
    }

    pub fn allow_origin_prefix(mut self, prefix: &str) -> Self {
        self.allow_origins.push(("prefix".to_string(), prefix.to_string()));
        self
    }

    pub fn allow_origin_regex(mut self, regex: &str) -> Self {
        self.allow_origins.push(("regex".to_string(), regex.to_string()));
        self
    }

    pub fn allow_methods(mut self, methods: Vec<&str>) -> Self {
        self.allow_methods = methods.into_iter().map(String::from).collect();
        self
    }

    pub fn allow_headers(mut self, headers: Vec<&str>) -> Self {
        self.allow_headers = headers.into_iter().map(String::from).collect();
        self
    }

    pub fn expose_headers(mut self, headers: Vec<&str>) -> Self {
        self.expose_headers = headers.into_iter().map(String::from).collect();
        self
    }

    pub fn max_age(mut self, seconds: u64) -> Self {
        self.max_age = Some(seconds);
        self
    }

    pub fn allow_credentials(mut self, allow: bool) -> Self {
        self.allow_credentials = allow;
        self
    }

    pub fn build(self) -> Value {
        let allow_origin: Vec<Value> =
            self.allow_origins.into_iter().map(|(t, v)| json!({"type": t, "value": v})).collect();

        let mut policy = json!({
            "allow_origin": allow_origin,
            "allow_methods": self.allow_methods,
            "allow_headers": self.allow_headers,
            "allow_credentials": self.allow_credentials
        });

        if !self.expose_headers.is_empty() {
            policy["expose_headers"] = json!(self.expose_headers);
        }

        if let Some(max_age) = self.max_age {
            policy["max_age"] = json!(max_age);
        }

        json!({
            "type": "cors",
            "config": {
                "policy": policy
            }
        })
    }
}

/// Builder for ext_authz filter configuration
pub struct ExtAuthzBuilder {
    cluster: String,
    timeout_seconds: u32,
    path_prefix: String,
    failure_mode_allow: bool,
    with_request_body: bool,
    max_request_bytes: Option<u32>,
    allow_partial_message: bool,
}

impl ExtAuthzBuilder {
    pub fn new(cluster: &str) -> Self {
        Self {
            cluster: cluster.to_string(),
            timeout_seconds: 5,
            path_prefix: "/auth".to_string(),
            failure_mode_allow: false,
            with_request_body: false,
            max_request_bytes: None,
            allow_partial_message: false,
        }
    }

    pub fn timeout_seconds(mut self, timeout: u32) -> Self {
        self.timeout_seconds = timeout;
        self
    }

    pub fn path_prefix(mut self, prefix: &str) -> Self {
        self.path_prefix = prefix.to_string();
        self
    }

    pub fn failure_mode_allow(mut self, allow: bool) -> Self {
        self.failure_mode_allow = allow;
        self
    }

    pub fn with_request_body(mut self, max_bytes: u32, allow_partial: bool) -> Self {
        self.with_request_body = true;
        self.max_request_bytes = Some(max_bytes);
        self.allow_partial_message = allow_partial;
        self
    }

    pub fn build(self) -> Value {
        let mut config = json!({
            "type": "ext_authz",
            "config": {
                "http_service": {
                    "server_uri": {
                        "cluster": self.cluster,
                        "uri": format!("http://authz{}", self.path_prefix),
                        "timeout_seconds": self.timeout_seconds
                    },
                    "path_prefix": self.path_prefix
                },
                "failure_mode_allow": self.failure_mode_allow
            }
        });

        if self.with_request_body {
            config["config"]["with_request_body"] = json!({
                "max_request_bytes": self.max_request_bytes,
                "allow_partial_message": self.allow_partial_message,
                "pack_as_bytes": false
            });
        }

        config
    }
}

/// Builder for compressor filter configuration (gzip)
#[derive(Default)]
pub struct CompressorBuilder {
    min_content_length: u32,
    content_types: Vec<String>,
    disable_on_etag_header: bool,
    remove_accept_encoding_header: bool,
    compression_level: String,
    window_bits: u32,
    memory_level: u32,
}

impl CompressorBuilder {
    pub fn new() -> Self {
        Self {
            min_content_length: 100,
            content_types: vec![
                "text/html".to_string(),
                "text/plain".to_string(),
                "text/css".to_string(),
                "application/json".to_string(),
                "application/javascript".to_string(),
            ],
            disable_on_etag_header: false,
            remove_accept_encoding_header: true,
            compression_level: "DEFAULT_COMPRESSION".to_string(),
            window_bits: 15,
            memory_level: 8,
        }
    }

    pub fn min_content_length(mut self, length: u32) -> Self {
        self.min_content_length = length;
        self
    }

    pub fn content_types(mut self, types: Vec<&str>) -> Self {
        self.content_types = types.into_iter().map(String::from).collect();
        self
    }

    pub fn compression_level(mut self, level: &str) -> Self {
        self.compression_level = level.to_string();
        self
    }

    pub fn build(self) -> Value {
        json!({
            "type": "compressor",
            "config": {
                "response_direction_config": {
                    "common_config": {
                        "min_content_length": self.min_content_length,
                        "content_type": self.content_types,
                        "enabled": {
                            "default_value": true,
                            "runtime_key": "compression_enabled"
                        }
                    },
                    "disable_on_etag_header": self.disable_on_etag_header,
                    "remove_accept_encoding_header": self.remove_accept_encoding_header
                },
                "compressor_library": {
                    "type": "gzip",
                    "config": {
                        "compression_level": self.compression_level,
                        "window_bits": self.window_bits,
                        "memory_level": self.memory_level
                    }
                }
            }
        })
    }
}

/// Builder for custom_response filter configuration
pub struct CustomResponseBuilder {
    matchers: Vec<CustomResponseMatcher>,
}

pub struct CustomResponseMatcher {
    status_code_match: u16,
    body: String,
    content_type: String,
    status_code_override: Option<u16>,
}

impl CustomResponseBuilder {
    pub fn new() -> Self {
        Self { matchers: Vec::new() }
    }

    pub fn add_matcher(
        mut self,
        status_code: u16,
        body: &str,
        content_type: &str,
        override_status: Option<u16>,
    ) -> Self {
        self.matchers.push(CustomResponseMatcher {
            status_code_match: status_code,
            body: body.to_string(),
            content_type: content_type.to_string(),
            status_code_override: override_status,
        });
        self
    }

    pub fn build(self) -> Value {
        let matchers: Vec<Value> = self
            .matchers
            .into_iter()
            .map(|m| {
                let mut matcher = json!({
                    "matcher": {
                        "status_code_matcher": {
                            "match_type": "exact",
                            "value": m.status_code_match
                        }
                    },
                    "response": {
                        "body": {
                            "inline_string": m.body
                        },
                        "content_type": m.content_type
                    }
                });
                if let Some(override_status) = m.status_code_override {
                    matcher["response"]["status_code"] = json!(override_status);
                }
                matcher
            })
            .collect();

        json!({
            "type": "custom_response",
            "config": {
                "custom_response_matchers": matchers
            }
        })
    }
}

impl Default for CustomResponseBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Builder for circuit_breakers cluster configuration
#[derive(Default)]
pub struct CircuitBreakerBuilder {
    max_connections: u32,
    max_pending_requests: u32,
    max_requests: u32,
    max_retries: u32,
}

impl CircuitBreakerBuilder {
    pub fn new() -> Self {
        Self { max_connections: 100, max_pending_requests: 100, max_requests: 1000, max_retries: 3 }
    }

    pub fn max_connections(mut self, max: u32) -> Self {
        self.max_connections = max;
        self
    }

    pub fn max_pending_requests(mut self, max: u32) -> Self {
        self.max_pending_requests = max;
        self
    }

    pub fn max_requests(mut self, max: u32) -> Self {
        self.max_requests = max;
        self
    }

    pub fn max_retries(mut self, max: u32) -> Self {
        self.max_retries = max;
        self
    }

    pub fn build(self) -> Value {
        json!({
            "default": {
                "maxConnections": self.max_connections,
                "maxPendingRequests": self.max_pending_requests,
                "maxRequests": self.max_requests,
                "maxRetries": self.max_retries
            }
        })
    }
}

/// Builder for outlier_detection cluster configuration
#[derive(Default)]
pub struct OutlierDetectionBuilder {
    consecutive_5xx: u32,
    interval_ms: u64,
    base_ejection_time_ms: u64,
    max_ejection_percent: u32,
    enforcing_consecutive_5xx: u32,
}

impl OutlierDetectionBuilder {
    pub fn new() -> Self {
        Self {
            consecutive_5xx: 5,
            interval_ms: 10000,
            base_ejection_time_ms: 30000,
            max_ejection_percent: 50,
            enforcing_consecutive_5xx: 100,
        }
    }

    pub fn consecutive_5xx(mut self, count: u32) -> Self {
        self.consecutive_5xx = count;
        self
    }

    pub fn interval_ms(mut self, ms: u64) -> Self {
        self.interval_ms = ms;
        self
    }

    pub fn base_ejection_time_ms(mut self, ms: u64) -> Self {
        self.base_ejection_time_ms = ms;
        self
    }

    pub fn max_ejection_percent(mut self, percent: u32) -> Self {
        self.max_ejection_percent = percent;
        self
    }

    pub fn build(self) -> Value {
        json!({
            "consecutive5xx": self.consecutive_5xx,
            "intervalMs": self.interval_ms,
            "baseEjectionTimeMs": self.base_ejection_time_ms,
            "maxEjectionPercent": self.max_ejection_percent,
            "enforcingConsecutive5xx": self.enforcing_consecutive_5xx
        })
    }
}

/// Builder for retry_policy route configuration
#[derive(Default)]
pub struct RetryPolicyBuilder {
    max_retries: u32,
    retry_on: Vec<String>,
    per_try_timeout_seconds: u32,
    base_interval_ms: u64,
    max_interval_ms: u64,
}

impl RetryPolicyBuilder {
    pub fn new() -> Self {
        Self {
            max_retries: 3,
            retry_on: vec!["5xx".to_string(), "reset".to_string(), "connect-failure".to_string()],
            per_try_timeout_seconds: 10,
            base_interval_ms: 100,
            max_interval_ms: 1000,
        }
    }

    pub fn max_retries(mut self, retries: u32) -> Self {
        self.max_retries = retries;
        self
    }

    pub fn retry_on(mut self, conditions: Vec<&str>) -> Self {
        self.retry_on = conditions.into_iter().map(String::from).collect();
        self
    }

    pub fn per_try_timeout_seconds(mut self, timeout: u32) -> Self {
        self.per_try_timeout_seconds = timeout;
        self
    }

    pub fn backoff(mut self, base_ms: u64, max_ms: u64) -> Self {
        self.base_interval_ms = base_ms;
        self.max_interval_ms = max_ms;
        self
    }

    pub fn build(self) -> Value {
        json!({
            "maxRetries": self.max_retries,
            "retryOn": self.retry_on,
            "perTryTimeoutSeconds": self.per_try_timeout_seconds,
            "backoff": {
                "baseIntervalMs": self.base_interval_ms,
                "maxIntervalMs": self.max_interval_ms
            }
        })
    }
}

// Convenience functions for creating builders
pub fn rate_limit() -> RateLimitBuilder {
    RateLimitBuilder::new()
}

pub fn header_mutation() -> HeaderMutationBuilder {
    HeaderMutationBuilder::new()
}

pub fn jwt_auth() -> JwtAuthBuilder {
    JwtAuthBuilder::new()
}

pub fn cors() -> CorsBuilder {
    CorsBuilder::new()
}

pub fn ext_authz(cluster: &str) -> ExtAuthzBuilder {
    ExtAuthzBuilder::new(cluster)
}

pub fn compressor() -> CompressorBuilder {
    CompressorBuilder::new()
}

pub fn custom_response() -> CustomResponseBuilder {
    CustomResponseBuilder::new()
}

pub fn circuit_breaker() -> CircuitBreakerBuilder {
    CircuitBreakerBuilder::new()
}

pub fn outlier_detection() -> OutlierDetectionBuilder {
    OutlierDetectionBuilder::new()
}

pub fn retry_policy() -> RetryPolicyBuilder {
    RetryPolicyBuilder::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rate_limit_builder() {
        let config = rate_limit().max_tokens(5).fill_interval_ms(30000).status_code(429).build();

        assert_eq!(config["type"], "local_rate_limit");
        assert_eq!(config["config"]["token_bucket"]["max_tokens"], 5);
        assert_eq!(config["config"]["status_code"], 429);
    }

    #[test]
    fn test_header_mutation_builder() {
        let config = header_mutation()
            .add_response_header("X-Custom", "value")
            .add_response_header("X-Another", "test")
            .build();

        assert!(config["response_headers_to_add"].is_array());
        let headers = config["response_headers_to_add"].as_array().unwrap();
        assert_eq!(headers.len(), 2);
    }

    #[test]
    fn test_cors_builder() {
        let config = cors()
            .allow_origin_exact("https://example.com")
            .allow_methods(vec!["GET", "POST"])
            .allow_credentials(true)
            .build();

        assert_eq!(config["type"], "cors");
        assert!(config["config"]["policy"]["allow_credentials"].as_bool().unwrap());
    }

    #[test]
    fn test_circuit_breaker_builder() {
        let config = circuit_breaker().max_connections(10).max_pending_requests(5).build();

        assert_eq!(config["default"]["maxConnections"], 10);
        assert_eq!(config["default"]["maxPendingRequests"], 5);
    }

    #[test]
    fn test_retry_policy_builder() {
        let config =
            retry_policy().max_retries(3).retry_on(vec!["5xx", "reset"]).backoff(100, 1000).build();

        assert_eq!(config["maxRetries"], 3);
        assert_eq!(config["backoff"]["baseIntervalMs"], 100);
    }
}
