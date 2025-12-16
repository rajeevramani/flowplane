//! JWT authentication HTTP filter configuration helpers

use crate::xds::filters::{any_from_message, invalid_config, Base64Bytes};
use envoy_types::pb::envoy::config::core::v3::{
    data_source, BackoffStrategy, DataSource, HttpUri, RetryPolicy,
};
use envoy_types::pb::envoy::extensions::filters::http::jwt_authn::v3::requirement_rule::RequirementType;
use envoy_types::pb::envoy::extensions::filters::http::jwt_authn::v3::{
    jwt_requirement, per_route_config, FilterStateRule, JwksAsyncFetch, JwtAuthentication,
    JwtCacheConfig, JwtClaimToHeader, JwtHeader, JwtProvider, JwtRequirement,
    JwtRequirementAndList, JwtRequirementOrList, PerRouteConfig, ProviderWithAudiences,
    RequirementRule,
};
use envoy_types::pb::envoy::r#type::matcher::v3::StringMatcher;
use envoy_types::pb::google::protobuf::{
    Any as EnvoyAny, Duration as ProtoDuration, Empty, UInt32Value,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use utoipa::ToSchema;

/// Top-level JWT authentication filter configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct JwtAuthenticationConfig {
    /// Ordered list of rules that map route matches to JWT requirements.
    #[serde(default)]
    pub rules: Vec<JwtRequirementRuleConfig>,
    /// Map of reusable requirement names.
    #[serde(default)]
    pub requirement_map: HashMap<String, JwtRequirementConfig>,
    /// JWT providers keyed by name.
    pub providers: HashMap<String, JwtProviderConfig>,
    /// Optional filter-level requirement selector driven by filter state.
    #[serde(default)]
    pub filter_state_rules: Option<JwtFilterStateRuleConfig>,
    /// Whether to bypass JWT checks on CORS preflight requests.
    #[serde(default)]
    pub bypass_cors_preflight: Option<bool>,
    /// Whether to strip WWW-Authenticate details on failure responses.
    #[serde(default)]
    pub strip_failure_response: Option<bool>,
    /// Optional statistics prefix override.
    #[serde(default)]
    pub stat_prefix: Option<String>,
}

impl JwtAuthenticationConfig {
    /// Convert configuration into Envoy Any payload
    pub fn to_any(&self) -> Result<EnvoyAny, crate::Error> {
        if self.providers.is_empty() {
            return Err(invalid_config(
                "JwtAuthentication configuration requires at least one provider",
            ));
        }

        let mut proto = JwtAuthentication {
            providers: self
                .providers
                .iter()
                .map(|(name, cfg)| cfg.to_proto().map(|proto| (name.clone(), proto)))
                .collect::<Result<_, _>>()?,
            ..Default::default()
        };

        if !self.rules.is_empty() {
            proto.rules = self
                .rules
                .iter()
                .map(JwtRequirementRuleConfig::to_proto)
                .collect::<Result<_, _>>()?;
        }

        if !self.requirement_map.is_empty() {
            proto.requirement_map = self
                .requirement_map
                .iter()
                .map(|(name, requirement)| {
                    requirement.to_proto().map(|proto| (name.clone(), proto))
                })
                .collect::<Result<_, _>>()?;
        }

        if let Some(filter_state_rules) = &self.filter_state_rules {
            proto.filter_state_rules = Some(filter_state_rules.to_proto()?);
        }

        proto.bypass_cors_preflight = self.bypass_cors_preflight.unwrap_or(false);
        proto.strip_failure_response = self.strip_failure_response.unwrap_or(false);
        proto.stat_prefix = self.stat_prefix.clone().unwrap_or_default();

        Ok(any_from_message(
            "type.googleapis.com/envoy.extensions.filters.http.jwt_authn.v3.JwtAuthentication",
            &proto,
        ))
    }
}

/// Per-route override for JWT authentication filter
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(untagged)]
pub enum JwtPerRouteConfig {
    /// Disable JWT authentication for this route scope
    Disabled { disabled: bool },
    /// Reference a named requirement from the top-level requirement_map
    RequirementName { requirement_name: String },
}

impl JwtPerRouteConfig {
    pub fn to_proto(&self) -> Result<PerRouteConfig, crate::Error> {
        let specifier = match self {
            JwtPerRouteConfig::Disabled { disabled } => {
                if !*disabled {
                    return Err(invalid_config(
                        "JwtAuthentication per-route disabled flag must be true when provided",
                    ));
                }
                Some(per_route_config::RequirementSpecifier::Disabled(true))
            }
            JwtPerRouteConfig::RequirementName { requirement_name } => {
                if requirement_name.trim().is_empty() {
                    return Err(invalid_config(
                        "JwtAuthentication per-route requirement_name cannot be empty",
                    ));
                }
                Some(per_route_config::RequirementSpecifier::RequirementName(
                    requirement_name.clone(),
                ))
            }
        };

        Ok(PerRouteConfig { requirement_specifier: specifier })
    }

    pub fn from_proto(proto: &PerRouteConfig) -> Result<Self, crate::Error> {
        match proto.requirement_specifier.as_ref() {
            Some(per_route_config::RequirementSpecifier::Disabled(value)) => {
                if !*value {
                    return Err(invalid_config(
                        "JwtAuthentication per-route Disabled must be true when set",
                    ));
                }
                Ok(JwtPerRouteConfig::Disabled { disabled: true })
            }
            Some(per_route_config::RequirementSpecifier::RequirementName(name)) => {
                Ok(JwtPerRouteConfig::RequirementName { requirement_name: name.clone() })
            }
            None => Err(invalid_config(
                "JwtAuthentication per-route config must specify disabled or requirement_name",
            )),
        }
    }
}

/// Route rule pairing a match condition with JWT requirement behaviour
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct JwtRequirementRuleConfig {
    /// Route match used to select the rule. If omitted the rule applies to all traffic.
    #[serde(default)]
    pub r#match: Option<crate::xds::route::RouteMatchConfig>,
    /// Inline requirement definition.
    #[serde(default)]
    pub requires: Option<JwtRequirementConfig>,
    /// Reference to a named requirement.
    #[serde(default)]
    pub requirement_name: Option<String>,
}

impl JwtRequirementRuleConfig {
    fn to_proto(&self) -> Result<RequirementRule, crate::Error> {
        if self.requires.is_some() && self.requirement_name.is_some() {
            return Err(invalid_config(
                "JwtAuthentication rule cannot specify both requires and requirement_name",
            ));
        }

        let requirement_type = if let Some(requirement) = &self.requires {
            Some(RequirementType::Requires(requirement.to_proto()?))
        } else if let Some(name) = &self.requirement_name {
            if name.trim().is_empty() {
                return Err(invalid_config(
                    "JwtAuthentication rule requirement_name cannot be empty",
                ));
            }
            Some(RequirementType::RequirementName(name.clone()))
        } else {
            None
        };

        let route_match = match &self.r#match {
            Some(matcher) => Some(matcher.to_envoy_route_match()?),
            None => None,
        };

        Ok(RequirementRule { r#match: route_match, requirement_type })
    }
}

/// Encapsulates supported requirement shapes
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JwtRequirementConfig {
    /// Require a single provider by name
    ProviderName { provider_name: String },
    /// Require a provider and override audiences
    ProviderWithAudiences { provider_name: String, audiences: Vec<String> },
    /// Logical OR of nested requirements
    #[schema(no_recursion)]
    RequiresAny { requirements: Vec<JwtRequirementConfig> },
    /// Logical AND of nested requirements
    #[schema(no_recursion)]
    RequiresAll { requirements: Vec<JwtRequirementConfig> },
    /// Allow requests with missing JWTs but reject invalid ones
    AllowMissing,
    /// Allow requests even if JWT is missing or invalid
    AllowMissingOrFailed,
}

impl JwtRequirementConfig {
    fn to_proto(&self) -> Result<JwtRequirement, crate::Error> {
        let requires_type = match self {
            JwtRequirementConfig::ProviderName { provider_name } => {
                if provider_name.trim().is_empty() {
                    return Err(invalid_config(
                        "JwtAuthentication requirement provider_name cannot be empty",
                    ));
                }
                Some(jwt_requirement::RequiresType::ProviderName(provider_name.clone()))
            }
            JwtRequirementConfig::ProviderWithAudiences { provider_name, audiences } => {
                if provider_name.trim().is_empty() {
                    return Err(invalid_config(
                        "JwtAuthentication requirement provider_name cannot be empty",
                    ));
                }
                Some(jwt_requirement::RequiresType::ProviderAndAudiences(ProviderWithAudiences {
                    provider_name: provider_name.clone(),
                    audiences: audiences.clone(),
                }))
            }
            JwtRequirementConfig::RequiresAny { requirements } => {
                if requirements.is_empty() {
                    return Err(invalid_config(
                        "JwtAuthentication requires_any must contain at least one requirement",
                    ));
                }
                Some(jwt_requirement::RequiresType::RequiresAny(JwtRequirementOrList {
                    requirements: requirements
                        .iter()
                        .map(JwtRequirementConfig::to_proto)
                        .collect::<Result<_, _>>()?,
                }))
            }
            JwtRequirementConfig::RequiresAll { requirements } => {
                if requirements.is_empty() {
                    return Err(invalid_config(
                        "JwtAuthentication requires_all must contain at least one requirement",
                    ));
                }
                Some(jwt_requirement::RequiresType::RequiresAll(JwtRequirementAndList {
                    requirements: requirements
                        .iter()
                        .map(JwtRequirementConfig::to_proto)
                        .collect::<Result<_, _>>()?,
                }))
            }
            JwtRequirementConfig::AllowMissing => {
                Some(jwt_requirement::RequiresType::AllowMissing(Empty::default()))
            }
            JwtRequirementConfig::AllowMissingOrFailed => {
                Some(jwt_requirement::RequiresType::AllowMissingOrFailed(Empty::default()))
            }
        };

        Ok(JwtRequirement { requires_type })
    }
}

/// Definition of per-provider behaviour
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, ToSchema)]
pub struct JwtProviderConfig {
    /// Optional issuer to match the token against
    #[serde(default)]
    pub issuer: Option<String>,
    /// Allowed audiences
    #[serde(default)]
    pub audiences: Vec<String>,
    /// Optional subject matcher
    #[serde(default)]
    pub subjects: Option<StringMatcherConfig>,
    /// Require JWT expiration claim to be present
    #[serde(default)]
    pub require_expiration: Option<bool>,
    /// Maximum lifetime (seconds) for accepted JWTs
    #[serde(default)]
    pub max_lifetime_seconds: Option<u64>,
    /// Tolerated clock skew when validating exp/nbf (seconds)
    #[serde(default)]
    pub clock_skew_seconds: Option<u32>,
    /// Whether to forward the JWT downstream after verification
    #[serde(default)]
    pub forward: Option<bool>,
    /// Header extraction hints
    #[serde(default)]
    pub from_headers: Vec<JwtHeaderConfig>,
    /// Query parameter extraction hints
    #[serde(default)]
    pub from_params: Vec<String>,
    /// Cookie extraction hints
    #[serde(default)]
    pub from_cookies: Vec<String>,
    /// Header to forward the JWT payload in base64url
    #[serde(default)]
    pub forward_payload_header: Option<String>,
    /// Whether to pad forwarded payload header value
    #[serde(default)]
    pub pad_forward_payload_header: Option<bool>,
    /// Metadata namespace key for JWT payload
    #[serde(default)]
    pub payload_in_metadata: Option<String>,
    /// Metadata namespace key for JWT header
    #[serde(default)]
    pub header_in_metadata: Option<String>,
    /// Metadata namespace key for failure status info
    #[serde(default)]
    pub failed_status_in_metadata: Option<String>,
    /// Normalization options when writing payload to metadata
    #[serde(default)]
    pub normalize_payload_in_metadata: Option<JwtNormalizePayloadConfig>,
    /// Cache behaviour for the provider
    #[serde(default)]
    pub jwt_cache_config: Option<JwtCacheProviderConfig>,
    /// Claim forwarding configuration
    #[serde(default)]
    pub claim_to_headers: Vec<JwtClaimToHeaderConfig>,
    /// Whether routing cache should be cleared when metadata is updated
    #[serde(default)]
    pub clear_route_cache: Option<bool>,
    /// JWKS source specification (remote or local)
    #[serde(default)]
    pub jwks: JwtJwksSourceConfig,
}

impl JwtProviderConfig {
    fn to_proto(&self) -> Result<JwtProvider, crate::Error> {
        for header in &self.from_headers {
            if header.name.trim().is_empty() {
                return Err(invalid_config("JwtAuthentication from_headers.name cannot be empty"));
            }
        }

        for claim in &self.claim_to_headers {
            if claim.header_name.trim().is_empty() {
                return Err(invalid_config(
                    "JwtAuthentication claim_to_headers.header_name cannot be empty",
                ));
            }
            if claim.claim_name.trim().is_empty() {
                return Err(invalid_config(
                    "JwtAuthentication claim_to_headers.claim_name cannot be empty",
                ));
            }
        }

        let payload_in_metadata = if let Some(key) = &self.payload_in_metadata {
            if key.trim().is_empty() {
                return Err(invalid_config(
                    "JwtAuthentication payload_in_metadata cannot be empty",
                ));
            }
            key.clone()
        } else {
            String::new()
        };

        let header_in_metadata = if let Some(key) = &self.header_in_metadata {
            if key.trim().is_empty() {
                return Err(invalid_config("JwtAuthentication header_in_metadata cannot be empty"));
            }
            key.clone()
        } else {
            String::new()
        };

        let failed_status_in_metadata = if let Some(key) = &self.failed_status_in_metadata {
            if key.trim().is_empty() {
                return Err(invalid_config(
                    "JwtAuthentication failed_status_in_metadata cannot be empty",
                ));
            }
            key.clone()
        } else {
            String::new()
        };

        let mut proto = JwtProvider {
            issuer: self.issuer.clone().unwrap_or_default(),
            audiences: self.audiences.clone(),
            forward: self.forward.unwrap_or(false),
            from_headers: self
                .from_headers
                .iter()
                .map(|header| JwtHeader {
                    name: header.name.clone(),
                    value_prefix: header.value_prefix.clone().unwrap_or_default(),
                })
                .collect(),
            from_params: self.from_params.clone(),
            from_cookies: self.from_cookies.clone(),
            forward_payload_header: self.forward_payload_header.clone().unwrap_or_default(),
            pad_forward_payload_header: self.pad_forward_payload_header.unwrap_or(false),
            payload_in_metadata,
            header_in_metadata,
            failed_status_in_metadata,
            claim_to_headers: self
                .claim_to_headers
                .iter()
                .map(|claim| JwtClaimToHeader {
                    header_name: claim.header_name.clone(),
                    claim_name: claim.claim_name.clone(),
                })
                .collect(),
            clear_route_cache: self.clear_route_cache.unwrap_or(false),
            clock_skew_seconds: self.clock_skew_seconds.unwrap_or(60),
            ..Default::default()
        };

        if let Some(subjects) = &self.subjects {
            proto.subjects = Some(subjects.to_proto()?);
        }

        proto.require_expiration = self.require_expiration.unwrap_or(false);

        if let Some(max_lifetime_seconds) = self.max_lifetime_seconds {
            proto.max_lifetime =
                Some(ProtoDuration { seconds: max_lifetime_seconds as i64, nanos: 0 });
        }

        match &self.jwks {
            JwtJwksSourceConfig::Remote(remote) => {
                proto.jwks_source_specifier = Some(
                    envoy_types::pb::envoy::extensions::filters::http::jwt_authn::v3::jwt_provider::JwksSourceSpecifier::RemoteJwks(
                        remote.to_proto()?,
                    ),
                );
            }
            JwtJwksSourceConfig::Local(local) => {
                proto.jwks_source_specifier = Some(
                    envoy_types::pb::envoy::extensions::filters::http::jwt_authn::v3::jwt_provider::JwksSourceSpecifier::LocalJwks(
                        local.to_data_source()?,
                    ),
                );
            }
        }

        if let Some(cache) = &self.jwt_cache_config {
            proto.jwt_cache_config = Some(JwtCacheConfig {
                jwt_cache_size: cache.jwt_cache_size.unwrap_or(100),
                jwt_max_token_size: cache.jwt_max_token_size.unwrap_or(4096),
            });
        }

        if let Some(normalize) = &self.normalize_payload_in_metadata {
            proto.normalize_payload_in_metadata = Some(normalize.to_proto());
        }

        Ok(proto)
    }
}

/// Normalized payload options
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct JwtNormalizePayloadConfig {
    /// Claims that should be split by space into arrays
    #[serde(default)]
    pub space_delimited_claims: Vec<String>,
}

impl JwtNormalizePayloadConfig {
    fn to_proto(&self) -> envoy_types::pb::envoy::extensions::filters::http::jwt_authn::v3::jwt_provider::NormalizePayload{
        envoy_types::pb::envoy::extensions::filters::http::jwt_authn::v3::jwt_provider::NormalizePayload {
            space_delimited_claims: self.space_delimited_claims.clone(),
        }
    }
}

/// Provider-level JWT cache configuration
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct JwtCacheProviderConfig {
    #[serde(default)]
    pub jwt_cache_size: Option<u32>,
    #[serde(default)]
    pub jwt_max_token_size: Option<u32>,
}

/// JWT header extraction hint
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct JwtHeaderConfig {
    pub name: String,
    #[serde(default)]
    pub value_prefix: Option<String>,
}

/// JWT claim to header projection
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct JwtClaimToHeaderConfig {
    pub header_name: String,
    pub claim_name: String,
}

/// JWKS source configuration options
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JwtJwksSourceConfig {
    Remote(RemoteJwksConfig),
    Local(LocalJwksConfig),
}

impl Default for JwtJwksSourceConfig {
    fn default() -> Self {
        Self::Remote(RemoteJwksConfig::default())
    }
}

/// Remote JWKS configuration
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RemoteJwksConfig {
    pub http_uri: RemoteJwksHttpUriConfig,
    #[serde(default)]
    pub cache_duration_seconds: Option<u64>,
    #[serde(default)]
    pub async_fetch: Option<JwksAsyncFetchConfig>,
    #[serde(default)]
    pub retry_policy: Option<JwksRetryPolicyConfig>,
}

impl Default for RemoteJwksConfig {
    fn default() -> Self {
        Self {
            http_uri: RemoteJwksHttpUriConfig {
                uri: String::new(),
                cluster: String::new(),
                timeout_ms: 1000,
            },
            cache_duration_seconds: None,
            async_fetch: None,
            retry_policy: None,
        }
    }
}

impl RemoteJwksConfig {
    fn to_proto(
        &self,
    ) -> Result<
        envoy_types::pb::envoy::extensions::filters::http::jwt_authn::v3::RemoteJwks,
        crate::Error,
    > {
        let http_uri = self.http_uri.to_proto()?;
        let cache_duration = self
            .cache_duration_seconds
            .map(|seconds| ProtoDuration { seconds: seconds as i64, nanos: 0 });

        Ok(envoy_types::pb::envoy::extensions::filters::http::jwt_authn::v3::RemoteJwks {
            http_uri: Some(http_uri),
            cache_duration,
            async_fetch: self.async_fetch.as_ref().map(JwksAsyncFetchConfig::to_proto),
            retry_policy: self.retry_policy.as_ref().map(|policy| policy.to_proto()),
        })
    }
}

/// Remote JWKS HTTP URI definition
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RemoteJwksHttpUriConfig {
    pub uri: String,
    pub cluster: String,
    #[serde(default = "RemoteJwksHttpUriConfig::default_timeout_ms")]
    pub timeout_ms: u64,
}

impl RemoteJwksHttpUriConfig {
    const fn default_timeout_ms() -> u64 {
        1000
    }

    fn to_proto(&self) -> Result<HttpUri, crate::Error> {
        if self.uri.trim().is_empty() {
            return Err(invalid_config("JwtAuthentication remote_jwks.uri cannot be empty"));
        }
        if self.cluster.trim().is_empty() {
            return Err(invalid_config("JwtAuthentication remote_jwks.cluster cannot be empty"));
        }

        Ok(HttpUri {
            uri: self.uri.clone(),
            timeout: Some(ProtoDuration {
                seconds: (self.timeout_ms / 1000) as i64,
                nanos: ((self.timeout_ms % 1000) * 1_000_000) as i32,
            }),
            http_upstream_type: Some(
                envoy_types::pb::envoy::config::core::v3::http_uri::HttpUpstreamType::Cluster(
                    self.cluster.clone(),
                ),
            ),
        })
    }
}

/// Remote JWKS async fetch configuration
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct JwksAsyncFetchConfig {
    #[serde(default)]
    pub fast_listener: Option<bool>,
    #[serde(default)]
    pub failed_refetch_duration_seconds: Option<u64>,
}

impl JwksAsyncFetchConfig {
    fn to_proto(&self) -> JwksAsyncFetch {
        JwksAsyncFetch {
            fast_listener: self.fast_listener.unwrap_or(false),
            failed_refetch_duration: self
                .failed_refetch_duration_seconds
                .map(|seconds| ProtoDuration { seconds: seconds as i64, nanos: 0 }),
        }
    }
}

/// Remote JWKS retry policy configuration
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct JwksRetryPolicyConfig {
    #[serde(default)]
    pub num_retries: Option<u32>,
    #[serde(default)]
    pub retry_backoff: Option<RetryBackoffConfig>,
}

impl JwksRetryPolicyConfig {
    fn to_proto(&self) -> RetryPolicy {
        RetryPolicy {
            retry_back_off: self.retry_backoff.as_ref().map(|backoff| backoff.to_proto()),
            num_retries: self.num_retries.map(|value| UInt32Value { value }),
            retry_on: String::new(),
            retry_priority: None,
            retry_host_predicate: Vec::new(),
            host_selection_retry_max_attempts: 0,
        }
    }
}

/// Retry backoff configuration
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RetryBackoffConfig {
    #[serde(default)]
    pub base_interval_ms: Option<u64>,
    #[serde(default)]
    pub max_interval_ms: Option<u64>,
}

impl RetryBackoffConfig {
    fn to_proto(&self) -> BackoffStrategy {
        BackoffStrategy {
            base_interval: self.base_interval_ms.map(|ms| ProtoDuration {
                seconds: (ms / 1000) as i64,
                nanos: ((ms % 1000) * 1_000_000) as i32,
            }),
            max_interval: self.max_interval_ms.map(|ms| ProtoDuration {
                seconds: (ms / 1000) as i64,
                nanos: ((ms % 1000) * 1_000_000) as i32,
            }),
        }
    }
}

/// Local JWKS configuration
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, ToSchema)]
pub struct LocalJwksConfig {
    #[serde(default)]
    pub filename: Option<String>,
    #[serde(default)]
    pub inline_string: Option<String>,
    #[serde(default)]
    pub inline_bytes: Option<Base64Bytes>,
    #[serde(default)]
    pub environment_variable: Option<String>,
}

impl LocalJwksConfig {
    fn to_data_source(&self) -> Result<DataSource, crate::Error> {
        let mut specifier_count = 0;
        let mut specifier: Option<data_source::Specifier> = None;

        if let Some(filename) = &self.filename {
            if !filename.trim().is_empty() {
                specifier = Some(data_source::Specifier::Filename(filename.clone()));
                specifier_count += 1;
            }
        }
        if let Some(inline_string) = &self.inline_string {
            if !inline_string.is_empty() {
                specifier = Some(data_source::Specifier::InlineString(inline_string.clone()));
                specifier_count += 1;
            }
        }
        if let Some(inline_bytes) = &self.inline_bytes {
            if !inline_bytes.0.is_empty() {
                specifier = Some(data_source::Specifier::InlineBytes(inline_bytes.0.clone()));
                specifier_count += 1;
            }
        }
        if let Some(env) = &self.environment_variable {
            if !env.trim().is_empty() {
                specifier = Some(data_source::Specifier::EnvironmentVariable(env.clone()));
                specifier_count += 1;
            }
        }

        if specifier_count != 1 {
            return Err(invalid_config(
                "JwtAuthentication local_jwks requires exactly one source (filename, inline_string, inline_bytes, or environment_variable)",
            ));
        }

        Ok(DataSource { specifier, watched_directory: None })
    }
}

/// Filter state rule configuration
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct JwtFilterStateRuleConfig {
    pub name: String,
    pub requires: HashMap<String, JwtRequirementConfig>,
}

impl JwtFilterStateRuleConfig {
    fn to_proto(&self) -> Result<FilterStateRule, crate::Error> {
        if self.name.trim().is_empty() {
            return Err(invalid_config(
                "JwtAuthentication filter_state_rules.name cannot be empty",
            ));
        }
        if self.requires.is_empty() {
            return Err(invalid_config(
                "JwtAuthentication filter_state_rules.requires cannot be empty",
            ));
        }

        Ok(FilterStateRule {
            name: self.name.clone(),
            requires: self
                .requires
                .iter()
                .map(|(key, requirement)| requirement.to_proto().map(|proto| (key.clone(), proto)))
                .collect::<Result<_, _>>()?,
        })
    }
}

/// Wrapper for StringMatcher configuration
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StringMatcherConfig {
    Exact { value: String },
    Prefix { value: String },
    Suffix { value: String },
    Contains { value: String },
    Regex { value: String },
}

impl StringMatcherConfig {
    fn to_proto(&self) -> Result<StringMatcher, crate::Error> {
        let mut matcher = StringMatcher { ignore_case: false, ..Default::default() };

        match self {
            StringMatcherConfig::Exact { value } => {
                matcher.match_pattern = Some(envoy_types::pb::envoy::r#type::matcher::v3::string_matcher::MatchPattern::Exact(value.clone()));
            }
            StringMatcherConfig::Prefix { value } => {
                matcher.match_pattern = Some(envoy_types::pb::envoy::r#type::matcher::v3::string_matcher::MatchPattern::Prefix(value.clone()));
            }
            StringMatcherConfig::Suffix { value } => {
                matcher.match_pattern = Some(envoy_types::pb::envoy::r#type::matcher::v3::string_matcher::MatchPattern::Suffix(value.clone()));
            }
            StringMatcherConfig::Contains { value } => {
                matcher.match_pattern = Some(envoy_types::pb::envoy::r#type::matcher::v3::string_matcher::MatchPattern::Contains(value.clone()));
            }
            StringMatcherConfig::Regex { value } => {
                matcher.match_pattern = Some(envoy_types::pb::envoy::r#type::matcher::v3::string_matcher::MatchPattern::SafeRegex(
                    envoy_types::pb::envoy::r#type::matcher::v3::RegexMatcher {
                        regex: value.clone(),
                        ..Default::default()
                    },
                ));
            }
        }

        Ok(matcher)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_provider() -> JwtProviderConfig {
        JwtProviderConfig {
            issuer: Some("https://issuer.example.com".into()),
            audiences: vec!["aud1".into(), "aud2".into()],
            subjects: Some(StringMatcherConfig::Prefix { value: "spiffe://example.com/".into() }),
            require_expiration: Some(true),
            max_lifetime_seconds: Some(3600),
            clock_skew_seconds: Some(30),
            forward: Some(false),
            from_headers: vec![JwtHeaderConfig {
                name: "Authorization".into(),
                value_prefix: Some("Bearer ".into()),
            }],
            from_params: vec!["access_token".into()],
            from_cookies: vec!["jwt".into()],
            forward_payload_header: Some("x-jwt-payload".into()),
            pad_forward_payload_header: Some(true),
            payload_in_metadata: Some("payload".into()),
            header_in_metadata: Some("header".into()),
            failed_status_in_metadata: Some("status".into()),
            normalize_payload_in_metadata: Some(JwtNormalizePayloadConfig {
                space_delimited_claims: vec!["scope".into()],
            }),
            jwt_cache_config: Some(JwtCacheProviderConfig {
                jwt_cache_size: Some(500),
                jwt_max_token_size: Some(8192),
            }),
            claim_to_headers: vec![JwtClaimToHeaderConfig {
                header_name: "x-jwt-sub".into(),
                claim_name: "sub".into(),
            }],
            clear_route_cache: Some(true),
            jwks: JwtJwksSourceConfig::Remote(RemoteJwksConfig {
                http_uri: RemoteJwksHttpUriConfig {
                    uri: "https://issuer.example.com/.well-known/jwks.json".into(),
                    cluster: "jwks_cluster".into(),
                    timeout_ms: 1500,
                },
                cache_duration_seconds: Some(600),
                async_fetch: Some(JwksAsyncFetchConfig {
                    fast_listener: Some(true),
                    failed_refetch_duration_seconds: Some(5),
                }),
                retry_policy: Some(JwksRetryPolicyConfig {
                    num_retries: Some(3),
                    retry_backoff: Some(RetryBackoffConfig {
                        base_interval_ms: Some(200),
                        max_interval_ms: Some(2000),
                    }),
                }),
            }),
        }
    }

    #[test]
    fn builds_provider_proto() {
        let provider = sample_provider();
        let proto = provider.to_proto().expect("to_proto");
        assert_eq!(proto.issuer, "https://issuer.example.com");
        assert!(proto.subjects.is_some());
        assert_eq!(proto.audiences.len(), 2);
        assert!(proto.normalize_payload_in_metadata.is_some());
        assert!(proto.jwt_cache_config.is_some());
        assert!(proto.clear_route_cache);
        assert!(matches!(
            proto.jwks_source_specifier,
            Some(
                envoy_types::pb::envoy::extensions::filters::http::jwt_authn::v3::jwt_provider::JwksSourceSpecifier::RemoteJwks(_)
            )
        ));
    }

    #[test]
    fn builds_requirement_proto() {
        let requirement = JwtRequirementConfig::RequiresAll {
            requirements: vec![
                JwtRequirementConfig::ProviderName { provider_name: "primary".into() },
                JwtRequirementConfig::RequiresAny {
                    requirements: vec![
                        JwtRequirementConfig::ProviderName { provider_name: "secondary".into() },
                        JwtRequirementConfig::AllowMissing,
                    ],
                },
            ],
        };

        let proto = requirement.to_proto().expect("to_proto");
        assert!(matches!(proto.requires_type, Some(jwt_requirement::RequiresType::RequiresAll(_))));
    }

    #[test]
    fn builds_filter_any() {
        let config = JwtAuthenticationConfig {
            rules: vec![JwtRequirementRuleConfig {
                r#match: None,
                requires: Some(JwtRequirementConfig::ProviderName {
                    provider_name: "primary".into(),
                }),
                requirement_name: None,
            }],
            requirement_map: HashMap::new(),
            providers: HashMap::from([(String::from("primary"), sample_provider())]),
            filter_state_rules: Some(JwtFilterStateRuleConfig {
                name: "selector".into(),
                requires: HashMap::from([(
                    String::from("issuer1"),
                    JwtRequirementConfig::ProviderName { provider_name: "primary".into() },
                )]),
            }),
            bypass_cors_preflight: Some(true),
            strip_failure_response: Some(true),
            stat_prefix: Some("jwt_auth".into()),
        };

        let any = config.to_any().expect("to_any");
        assert_eq!(
            any.type_url,
            "type.googleapis.com/envoy.extensions.filters.http.jwt_authn.v3.JwtAuthentication"
        );
        assert!(!any.value.is_empty());
    }

    #[test]
    fn per_route_disabled_requires_true() {
        let cfg = JwtPerRouteConfig::Disabled { disabled: true };
        let proto = cfg.to_proto().expect("to_proto");
        assert!(matches!(
            proto.requirement_specifier,
            Some(per_route_config::RequirementSpecifier::Disabled(true))
        ));
    }
}
