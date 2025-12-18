//! Custom Response HTTP filter configuration helpers
//!
//! This module provides configuration for Envoy's custom response filter,
//! which allows defining custom response policies based on matcher trees.

use crate::xds::filters::{any_from_message, invalid_config, Base64Bytes, TypedConfig};
// Re-export for use in mod.rs from_any()
pub use envoy_types::pb::envoy::extensions::filters::http::custom_response::v3::CustomResponse as CustomResponseProto;
use envoy_types::pb::google::protobuf::Any as EnvoyAny;
use prost::Message;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use utoipa::ToSchema;

const CUSTOM_RESPONSE_TYPE_URL: &str =
    "type.googleapis.com/envoy.extensions.filters.http.custom_response.v3.CustomResponse";

/// Status code matcher for custom response rules
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StatusCodeMatcher {
    /// Match an exact status code (e.g., 400)
    Exact { code: u16 },
    /// Match a range of status codes (e.g., 400-499 for all 4xx errors)
    Range { min: u16, max: u16 },
    /// Match any of the specified status codes
    List { codes: Vec<u16> },
}

impl StatusCodeMatcher {
    /// Validate the status code matcher
    pub fn validate(&self) -> Result<(), crate::Error> {
        match self {
            StatusCodeMatcher::Exact { code } => {
                if !(100..=599).contains(code) {
                    return Err(invalid_config(format!(
                        "Status code {} out of valid range (100-599)",
                        code
                    )));
                }
            }
            StatusCodeMatcher::Range { min, max } => {
                if !(100..=599).contains(min) {
                    return Err(invalid_config(format!(
                        "Min status code {} out of valid range (100-599)",
                        min
                    )));
                }
                if !(100..=599).contains(max) {
                    return Err(invalid_config(format!(
                        "Max status code {} out of valid range (100-599)",
                        max
                    )));
                }
                if min > max {
                    return Err(invalid_config(format!(
                        "Min status code {} cannot be greater than max {}",
                        min, max
                    )));
                }
            }
            StatusCodeMatcher::List { codes } => {
                if codes.is_empty() {
                    return Err(invalid_config("Status code list cannot be empty"));
                }
                for code in codes {
                    if !(100..=599).contains(code) {
                        return Err(invalid_config(format!(
                            "Status code {} out of valid range (100-599)",
                            code
                        )));
                    }
                }
            }
        }
        Ok(())
    }
}

/// Local response policy for custom responses
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
pub struct LocalResponsePolicy {
    /// Override status code (optional, defaults to original status code)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
    /// Response body to return
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// Additional response headers to add
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub headers: HashMap<String, String>,
}

impl LocalResponsePolicy {
    /// Validate the local response policy
    pub fn validate(&self) -> Result<(), crate::Error> {
        if let Some(code) = self.status_code {
            if !(100..=599).contains(&code) {
                return Err(invalid_config(format!(
                    "Response status code {} out of valid range (100-599)",
                    code
                )));
            }
        }

        // Validate that if a body is provided, it's not empty
        if let Some(body) = &self.body {
            if body.is_empty() {
                return Err(invalid_config("Response body cannot be empty string"));
            }
        }

        // Validate header names are not empty
        for (key, value) in &self.headers {
            if key.is_empty() {
                return Err(invalid_config("Header name cannot be empty"));
            }
            if value.is_empty() {
                return Err(invalid_config(format!("Header value for '{}' cannot be empty", key)));
            }
        }

        Ok(())
    }

    /// Create a standard JSON error response
    pub fn json_error(status_code: u16, error_message: &str) -> Self {
        let body = serde_json::json!({
            "error": error_message,
            "status_code": status_code,
        })
        .to_string();

        let mut headers = HashMap::new();
        headers.insert("content-type".to_string(), "application/json".to_string());

        Self { status_code: Some(status_code), body: Some(body), headers }
    }
}

/// Custom response matcher rule combining a matcher and response policy
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
pub struct ResponseMatcherRule {
    /// Status code matcher
    pub status_code: StatusCodeMatcher,
    /// Response policy to apply when matched
    pub response: LocalResponsePolicy,
}

impl ResponseMatcherRule {
    /// Validate the matcher rule
    pub fn validate(&self) -> Result<(), crate::Error> {
        self.status_code.validate()?;
        self.response.validate()?;
        Ok(())
    }
}

/// Configuration for custom response filter
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Default)]
pub struct CustomResponseConfig {
    /// User-friendly matcher rules (preferred)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub matchers: Vec<ResponseMatcherRule>,

    /// Legacy base64 protobuf matcher (for backward compatibility)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_response_matcher: Option<MatcherConfig>,
}

/// Matcher configuration for custom responses
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct MatcherConfig {
    /// Typed matcher configuration
    #[serde(flatten)]
    pub config: TypedConfig,
}

impl CustomResponseConfig {
    /// Validate configuration
    pub fn validate(&self) -> Result<(), crate::Error> {
        // Validate all matcher rules
        for rule in &self.matchers {
            rule.validate()?;
        }

        // Ensure either user-friendly matchers OR legacy config, not both
        if !self.matchers.is_empty() && self.custom_response_matcher.is_some() {
            return Err(invalid_config(
                "Cannot use both 'matchers' and 'custom_response_matcher' fields. Use 'matchers' for user-friendly configuration.",
            ));
        }

        Ok(())
    }

    /// Convert to Envoy Any payload
    pub fn to_any(&self) -> Result<EnvoyAny, crate::Error> {
        use prost::Message;

        self.validate()?;

        let custom_response_matcher = if !self.matchers.is_empty() {
            // Build matcher tree from user-friendly matchers
            Some(self.build_envoy_matcher()?)
        } else {
            // Use legacy base64 protobuf matcher (backward compatibility)
            self.custom_response_matcher.as_ref().map(|m| {
                let any = m.config.to_any();
                envoy_types::pb::xds::r#type::matcher::v3::Matcher::decode(any.value.as_slice())
                    .unwrap_or_default()
            })
        };

        let proto = CustomResponseProto { custom_response_matcher };

        Ok(any_from_message(CUSTOM_RESPONSE_TYPE_URL, &proto))
    }

    /// Build Envoy matcher tree from user-friendly matcher rules
    fn build_envoy_matcher(
        &self,
    ) -> Result<envoy_types::pb::xds::r#type::matcher::v3::Matcher, crate::Error> {
        use envoy_types::pb::envoy::config::core::v3::HeaderValueOption;
        use envoy_types::pb::envoy::extensions::http::custom_response::local_response_policy::v3::LocalResponsePolicy as EnvoyLocalResponsePolicy;
        use envoy_types::pb::google::protobuf::UInt32Value;
        use envoy_types::pb::xds::core::v3::TypedExtensionConfig;
        use envoy_types::pb::xds::r#type::matcher::v3::matcher::OnMatch;
        use envoy_types::pb::xds::r#type::matcher::v3::{
            matcher, matcher::matcher_list::FieldMatcher, matcher::MatcherList, Matcher,
        };

        // For now, create a simple matcher that matches on response code
        // We'll use a matcher list with exact matches for each rule
        let mut matchers_vec = Vec::new();

        for rule in &self.matchers {
            // Build the response policy
            let local_response = EnvoyLocalResponsePolicy {
                status_code: rule.response.status_code.map(|c| UInt32Value { value: c as u32 }),
                body: rule.response.body.clone().map(|b| envoy_types::pb::envoy::config::core::v3::DataSource {
                    specifier: Some(envoy_types::pb::envoy::config::core::v3::data_source::Specifier::InlineString(b)),
                    watched_directory: None,
                }),
                body_format: None,
                response_headers_to_add: rule.response.headers.iter().map(|(k, v)| {
                    #[allow(deprecated)]
                    HeaderValueOption {
                        header: Some(envoy_types::pb::envoy::config::core::v3::HeaderValue {
                            key: k.clone(),
                            value: v.clone(),
                            raw_value: vec![],
                        }),
                        append: None,
                        append_action: 1, // APPEND_IF_EXISTS_OR_ADD
                        keep_empty_value: false,
                    }
                }).collect(),
            };

            // Wrap in TypedExtensionConfig
            let policy_any = any_from_message(
                "type.googleapis.com/envoy.extensions.http.custom_response.local_response_policy.v3.LocalResponsePolicy",
                &local_response,
            );

            let action = TypedExtensionConfig {
                name: "custom_response_action".to_string(),
                typed_config: Some(policy_any),
            };

            // Build predicate for status code matching
            let predicate = match &rule.status_code {
                StatusCodeMatcher::Exact { code } => self.build_exact_predicate(*code),
                StatusCodeMatcher::Range { min, max } => self.build_range_predicate(*min, *max),
                StatusCodeMatcher::List { codes } => self.build_list_predicate(codes)?,
            };

            matchers_vec.push(FieldMatcher {
                predicate: Some(predicate),
                on_match: Some(OnMatch {
                    on_match: Some(matcher::on_match::OnMatch::Action(action)),
                    keep_matching: false,
                }),
            });
        }

        Ok(Matcher {
            matcher_type: Some(matcher::MatcherType::MatcherList(MatcherList {
                matchers: matchers_vec,
            })),
            on_no_match: None,
        })
    }

    /// Build a single predicate for exact status code matching
    fn build_exact_predicate(
        &self,
        code: u16,
    ) -> envoy_types::pb::xds::r#type::matcher::v3::matcher::matcher_list::Predicate {
        use envoy_types::pb::xds::core::v3::TypedExtensionConfig;
        use envoy_types::pb::xds::r#type::matcher::v3::{
            matcher,
            matcher::matcher_list::predicate::single_predicate::Matcher as PredicateMatcher,
            matcher::matcher_list::predicate::SinglePredicate,
        };

        matcher::matcher_list::Predicate {
            match_type: Some(matcher::matcher_list::predicate::MatchType::SinglePredicate(
                SinglePredicate {
                    input: Some(TypedExtensionConfig {
                        name: "response_code_input".to_string(),
                        typed_config: Some(any_from_message(
                            "type.googleapis.com/envoy.type.matcher.v3.HttpResponseStatusCodeMatchInput",
                            &envoy_types::pb::envoy::r#type::matcher::v3::HttpResponseStatusCodeMatchInput {},
                        )),
                    }),
                    matcher: Some(PredicateMatcher::ValueMatch(
                        envoy_types::pb::xds::r#type::matcher::v3::StringMatcher {
                            match_pattern: Some(
                                envoy_types::pb::xds::r#type::matcher::v3::string_matcher::MatchPattern::Exact(
                                    code.to_string(),
                                ),
                            ),
                            ignore_case: false,
                        },
                    )),
                },
            )),
        }
    }

    /// Build a predicate for range status code matching (e.g., 400-499)
    /// Uses OR predicate to match any code in the range
    fn build_range_predicate(
        &self,
        min: u16,
        max: u16,
    ) -> envoy_types::pb::xds::r#type::matcher::v3::matcher::matcher_list::Predicate {
        use envoy_types::pb::xds::r#type::matcher::v3::matcher;

        // Build individual predicates for each code in range
        let predicates: Vec<_> = (min..=max).map(|code| self.build_exact_predicate(code)).collect();

        // Combine with OR predicate
        matcher::matcher_list::Predicate {
            match_type: Some(matcher::matcher_list::predicate::MatchType::OrMatcher(
                matcher::matcher_list::predicate::PredicateList { predicate: predicates },
            )),
        }
    }

    /// Build a predicate for list status code matching (e.g., [400, 401, 403])
    /// Uses OR predicate to match any code in the list
    fn build_list_predicate(
        &self,
        codes: &[u16],
    ) -> Result<
        envoy_types::pb::xds::r#type::matcher::v3::matcher::matcher_list::Predicate,
        crate::Error,
    > {
        use envoy_types::pb::xds::r#type::matcher::v3::matcher;

        if codes.is_empty() {
            return Err(invalid_config("Status code list cannot be empty"));
        }

        // Build individual predicates for each code in the list
        let predicates: Vec<_> =
            codes.iter().map(|&code| self.build_exact_predicate(code)).collect();

        // Combine with OR predicate
        Ok(matcher::matcher_list::Predicate {
            match_type: Some(matcher::matcher_list::predicate::MatchType::OrMatcher(
                matcher::matcher_list::predicate::PredicateList { predicate: predicates },
            )),
        })
    }

    /// Build configuration from Envoy proto
    pub fn from_proto(proto: &CustomResponseProto) -> Result<Self, crate::Error> {
        use prost::Message;

        let custom_response_matcher = proto.custom_response_matcher.as_ref().map(|matcher| {
            let mut buf = Vec::new();
            matcher.encode(&mut buf).ok();

            MatcherConfig {
                config: TypedConfig {
                    type_url: "type.googleapis.com/xds.type.matcher.v3.Matcher".into(),
                    value: Base64Bytes(buf),
                },
            }
        });

        let config = Self {
            matchers: Vec::new(), // TODO: Implement reverse conversion from protobuf to user-friendly matchers
            custom_response_matcher,
        };

        config.validate()?;
        Ok(config)
    }
}

/// Per-route custom response configuration
///
/// This allows disabling or customizing custom response behavior per-route.
/// Supports full per-route override with matchers (not just disabled flag).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Default)]
pub struct CustomResponsePerRouteConfig {
    /// Whether to disable custom response for this route
    #[serde(default)]
    pub disabled: bool,
    /// Route-specific matcher rules that override listener-level config
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub matchers: Vec<ResponseMatcherRule>,
}

impl CustomResponsePerRouteConfig {
    /// Create a per-route config from a listener-level CustomResponseConfig.
    ///
    /// This copies the matcher rules from the listener config to enable
    /// full per-route override support.
    pub fn from_listener_config(config: &CustomResponseConfig) -> Self {
        Self { disabled: false, matchers: config.matchers.clone() }
    }

    /// Validate per-route configuration
    pub fn validate(&self) -> Result<(), crate::Error> {
        // Validate all matcher rules
        for rule in &self.matchers {
            rule.validate()?;
        }
        Ok(())
    }

    /// Returns true if this config has route-specific matchers
    pub fn has_matchers(&self) -> bool {
        !self.matchers.is_empty()
    }

    /// Convert to Envoy Any payload for typed_per_filter_config
    ///
    /// Serializes as a CustomResponse proto directly. The `disabled` field is
    /// handled at the route level via Envoy's typed_per_filter_config mechanism.
    /// When disabled=true and no matchers, we still emit a valid CustomResponse.
    pub fn to_any(&self) -> Result<EnvoyAny, crate::Error> {
        self.validate()?;

        // Build a CustomResponse config from our matchers
        let cr_config =
            CustomResponseConfig { matchers: self.matchers.clone(), custom_response_matcher: None };

        // Use the same serialization as the listener config
        cr_config.to_any()
    }

    /// Build configuration from CustomResponse proto (used in from_any)
    pub fn from_custom_response_proto(proto: &CustomResponseProto) -> Result<Self, crate::Error> {
        let cr_config = CustomResponseConfig::from_proto(proto)?;
        let config = Self {
            disabled: false, // CustomResponse proto doesn't have a disabled field
            matchers: cr_config.matchers,
        };
        config.validate()?;
        Ok(config)
    }

    /// Build configuration from FilterConfig proto (legacy support)
    pub fn from_proto(
        proto: &envoy_types::pb::envoy::config::route::v3::FilterConfig,
    ) -> Result<Self, crate::Error> {
        let matchers = if let Some(config_any) = &proto.config {
            if config_any.type_url == CUSTOM_RESPONSE_TYPE_URL {
                let cr_proto =
                    CustomResponseProto::decode(config_any.value.as_slice()).map_err(|e| {
                        crate::Error::config(format!(
                            "Failed to decode embedded CustomResponse: {}",
                            e
                        ))
                    })?;
                let cr_config = CustomResponseConfig::from_proto(&cr_proto)?;
                cr_config.matchers
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        let config = Self { disabled: proto.disabled, matchers };

        config.validate()?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;

    #[test]
    fn builds_proto_with_no_matcher() {
        let config = CustomResponseConfig::default();
        let any = config.to_any().expect("to_any");
        assert_eq!(any.type_url, CUSTOM_RESPONSE_TYPE_URL);
        // Empty CustomResponse proto is valid and may serialize to empty bytes
    }

    #[test]
    fn builds_proto_with_legacy_matcher() {
        let config = CustomResponseConfig {
            matchers: vec![],
            custom_response_matcher: Some(MatcherConfig {
                config: TypedConfig {
                    type_url: "type.googleapis.com/xds.type.matcher.v3.Matcher".into(),
                    value: Base64Bytes(vec![1, 2, 3, 4]),
                },
            }),
        };

        let any = config.to_any().expect("to_any");
        assert_eq!(any.type_url, CUSTOM_RESPONSE_TYPE_URL);
        assert!(!any.value.is_empty());
    }

    #[test]
    fn proto_round_trip_no_matcher() {
        let config = CustomResponseConfig::default();
        let any = config.to_any().expect("to_any");

        let proto = CustomResponseProto::decode(any.value.as_slice()).expect("decode proto");
        let round_tripped = CustomResponseConfig::from_proto(&proto).expect("from_proto");

        assert!(round_tripped.custom_response_matcher.is_none());
        assert!(round_tripped.matchers.is_empty());
    }

    #[test]
    fn proto_round_trip_with_legacy_matcher() {
        let config = CustomResponseConfig {
            matchers: vec![],
            custom_response_matcher: Some(MatcherConfig {
                config: TypedConfig {
                    type_url: "type.googleapis.com/xds.type.matcher.v3.Matcher".into(),
                    value: Base64Bytes(vec![10, 20, 30]),
                },
            }),
        };

        let any = config.to_any().expect("to_any");
        let proto = CustomResponseProto::decode(any.value.as_slice()).expect("decode proto");
        let round_tripped = CustomResponseConfig::from_proto(&proto).expect("from_proto");

        assert!(round_tripped.custom_response_matcher.is_some());
        let matcher = round_tripped.custom_response_matcher.unwrap();
        assert_eq!(matcher.config.type_url, "type.googleapis.com/xds.type.matcher.v3.Matcher");
    }

    #[test]
    fn validates_successfully() {
        let config = CustomResponseConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn per_route_config_default() {
        let config = CustomResponsePerRouteConfig::default();
        assert!(!config.disabled);
        assert!(config.matchers.is_empty());
        assert!(config.validate().is_ok());
    }

    #[test]
    fn per_route_config_disabled() {
        // Note: disabled flag is stored in the config struct but not serialized to proto
        // (CustomResponse proto doesn't have a disabled field - that's handled at route level)
        let config = CustomResponsePerRouteConfig { disabled: true, matchers: vec![] };
        assert!(config.validate().is_ok());

        let any = config.to_any().expect("to_any");
        // Uses the CustomResponse type URL directly
        assert_eq!(
            any.type_url,
            "type.googleapis.com/envoy.extensions.filters.http.custom_response.v3.CustomResponse"
        );
    }

    #[test]
    fn per_route_proto_round_trip() {
        // Test round-trip using the CustomResponse proto
        let config = CustomResponsePerRouteConfig { disabled: false, matchers: vec![] };
        let any = config.to_any().expect("to_any");

        let proto = CustomResponseProto::decode(any.value.as_slice()).expect("decode proto");
        // Empty config should have no matcher
        assert!(proto.custom_response_matcher.is_none());

        let round_tripped =
            CustomResponsePerRouteConfig::from_custom_response_proto(&proto).expect("from_proto");
        assert!(!round_tripped.disabled);
        assert!(round_tripped.matchers.is_empty());
    }

    #[test]
    fn per_route_config_with_matchers() {
        let config = CustomResponsePerRouteConfig {
            disabled: false,
            matchers: vec![ResponseMatcherRule {
                status_code: StatusCodeMatcher::Exact { code: 429 },
                response: LocalResponsePolicy::json_error(429, "rate limited"),
            }],
        };
        assert!(config.validate().is_ok());
        assert!(config.has_matchers());

        let any = config.to_any().expect("to_any");
        assert_eq!(
            any.type_url,
            "type.googleapis.com/envoy.extensions.filters.http.custom_response.v3.CustomResponse"
        );
    }

    #[test]
    fn per_route_from_listener_config() {
        let listener_config = CustomResponseConfig {
            matchers: vec![ResponseMatcherRule {
                status_code: StatusCodeMatcher::Range { min: 500, max: 599 },
                response: LocalResponsePolicy::json_error(500, "server error"),
            }],
            custom_response_matcher: None,
        };

        let per_route = CustomResponsePerRouteConfig::from_listener_config(&listener_config);
        assert!(!per_route.disabled);
        assert_eq!(per_route.matchers.len(), 1);
        assert!(matches!(
            per_route.matchers[0].status_code,
            StatusCodeMatcher::Range { min: 500, max: 599 }
        ));
    }

    #[test]
    fn per_route_with_matchers_round_trip() {
        let config = CustomResponsePerRouteConfig {
            disabled: false,
            matchers: vec![ResponseMatcherRule {
                status_code: StatusCodeMatcher::Exact { code: 503 },
                response: LocalResponsePolicy::json_error(503, "service unavailable"),
            }],
        };

        let any = config.to_any().expect("to_any");
        let proto = CustomResponseProto::decode(any.value.as_slice()).expect("decode proto");

        // Should have matcher tree
        assert!(proto.custom_response_matcher.is_some());

        let round_tripped =
            CustomResponsePerRouteConfig::from_custom_response_proto(&proto).expect("from_proto");
        assert!(!round_tripped.disabled);
        // Note: matchers are converted to protobuf Matcher format during to_any(),
        // and CustomResponseConfig::from_proto() doesn't reverse-parse the matchers back.
        // This is a known limitation - the matchers end up in custom_response_matcher.
    }

    // New user-friendly matcher tests

    #[test]
    fn status_code_matcher_exact_validates() {
        let matcher = StatusCodeMatcher::Exact { code: 400 };
        assert!(matcher.validate().is_ok());

        let invalid = StatusCodeMatcher::Exact { code: 99 };
        assert!(invalid.validate().is_err());

        let invalid2 = StatusCodeMatcher::Exact { code: 600 };
        assert!(invalid2.validate().is_err());
    }

    #[test]
    fn status_code_matcher_range_validates() {
        let matcher = StatusCodeMatcher::Range { min: 400, max: 499 };
        assert!(matcher.validate().is_ok());

        let invalid = StatusCodeMatcher::Range { min: 500, max: 400 };
        assert!(invalid.validate().is_err());

        let invalid2 = StatusCodeMatcher::Range { min: 99, max: 200 };
        assert!(invalid2.validate().is_err());
    }

    #[test]
    fn status_code_matcher_list_validates() {
        let matcher = StatusCodeMatcher::List { codes: vec![400, 401, 403] };
        assert!(matcher.validate().is_ok());

        let invalid = StatusCodeMatcher::List { codes: vec![] };
        assert!(invalid.validate().is_err());

        let invalid2 = StatusCodeMatcher::List { codes: vec![400, 600] };
        assert!(invalid2.validate().is_err());
    }

    #[test]
    fn local_response_policy_validates() {
        let policy = LocalResponsePolicy {
            status_code: Some(400),
            body: Some("{\"error\": \"bad request\"}".to_string()),
            headers: HashMap::new(),
        };
        assert!(policy.validate().is_ok());

        let invalid =
            LocalResponsePolicy { status_code: Some(600), body: None, headers: HashMap::new() };
        assert!(invalid.validate().is_err());

        let invalid2 = LocalResponsePolicy {
            status_code: Some(400),
            body: Some("".to_string()), // Empty body
            headers: HashMap::new(),
        };
        assert!(invalid2.validate().is_err());
    }

    #[test]
    fn local_response_policy_json_error_helper() {
        let policy = LocalResponsePolicy::json_error(400, "bad request");

        assert_eq!(policy.status_code, Some(400));
        assert!(policy.body.is_some());
        assert!(policy.body.unwrap().contains("bad request"));
        assert!(policy.headers.contains_key("content-type"));
        assert_eq!(policy.headers.get("content-type").unwrap(), "application/json");
    }

    #[test]
    fn response_matcher_rule_validates() {
        let rule = ResponseMatcherRule {
            status_code: StatusCodeMatcher::Exact { code: 400 },
            response: LocalResponsePolicy::json_error(400, "bad request"),
        };
        assert!(rule.validate().is_ok());

        let invalid = ResponseMatcherRule {
            status_code: StatusCodeMatcher::Exact { code: 600 },
            response: LocalResponsePolicy::json_error(400, "bad request"),
        };
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn custom_response_config_with_user_friendly_matchers() {
        let config = CustomResponseConfig {
            matchers: vec![
                ResponseMatcherRule {
                    status_code: StatusCodeMatcher::Exact { code: 400 },
                    response: LocalResponsePolicy::json_error(400, "bad request"),
                },
                ResponseMatcherRule {
                    status_code: StatusCodeMatcher::Exact { code: 500 },
                    response: LocalResponsePolicy::json_error(500, "internal server error"),
                },
            ],
            custom_response_matcher: None,
        };

        assert!(config.validate().is_ok());
        let any = config.to_any().expect("to_any");
        assert_eq!(any.type_url, CUSTOM_RESPONSE_TYPE_URL);
        assert!(!any.value.is_empty());
    }

    #[test]
    fn custom_response_config_rejects_both_formats() {
        let config = CustomResponseConfig {
            matchers: vec![ResponseMatcherRule {
                status_code: StatusCodeMatcher::Exact { code: 400 },
                response: LocalResponsePolicy::json_error(400, "bad request"),
            }],
            custom_response_matcher: Some(MatcherConfig {
                config: TypedConfig {
                    type_url: "type.googleapis.com/xds.type.matcher.v3.Matcher".into(),
                    value: Base64Bytes(vec![1, 2, 3]),
                },
            }),
        };

        assert!(config.validate().is_err());
        let err_msg = format!("{}", config.validate().unwrap_err());
        assert!(err_msg.contains("Cannot use both"));
    }

    #[test]
    fn custom_response_config_serde_round_trip() {
        let config = CustomResponseConfig {
            matchers: vec![ResponseMatcherRule {
                status_code: StatusCodeMatcher::Exact { code: 400 },
                response: LocalResponsePolicy {
                    status_code: Some(400),
                    body: Some("{\"error\": \"bad request\"}".to_string()),
                    headers: {
                        let mut h = HashMap::new();
                        h.insert("content-type".to_string(), "application/json".to_string());
                        h
                    },
                },
            }],
            custom_response_matcher: None,
        };

        let json = serde_json::to_string(&config).expect("serialize");
        let round_tripped: CustomResponseConfig = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(round_tripped.matchers.len(), 1);
        assert_eq!(round_tripped.matchers[0].status_code, StatusCodeMatcher::Exact { code: 400 });
    }

    // Range and List matcher building tests

    #[test]
    fn custom_response_config_with_range_matcher() {
        let config = CustomResponseConfig {
            matchers: vec![ResponseMatcherRule {
                status_code: StatusCodeMatcher::Range { min: 500, max: 503 },
                response: LocalResponsePolicy::json_error(500, "server error"),
            }],
            custom_response_matcher: None,
        };

        assert!(config.validate().is_ok());
        let any = config.to_any().expect("to_any should succeed for range matcher");
        assert_eq!(any.type_url, CUSTOM_RESPONSE_TYPE_URL);
        assert!(!any.value.is_empty());

        // Verify the proto contains a valid matcher structure
        let proto = CustomResponseProto::decode(any.value.as_slice()).expect("decode proto");
        assert!(proto.custom_response_matcher.is_some());
    }

    #[test]
    fn custom_response_config_with_list_matcher() {
        let config = CustomResponseConfig {
            matchers: vec![ResponseMatcherRule {
                status_code: StatusCodeMatcher::List { codes: vec![400, 401, 403, 429] },
                response: LocalResponsePolicy::json_error(400, "client error"),
            }],
            custom_response_matcher: None,
        };

        assert!(config.validate().is_ok());
        let any = config.to_any().expect("to_any should succeed for list matcher");
        assert_eq!(any.type_url, CUSTOM_RESPONSE_TYPE_URL);
        assert!(!any.value.is_empty());

        // Verify the proto contains a valid matcher structure
        let proto = CustomResponseProto::decode(any.value.as_slice()).expect("decode proto");
        assert!(proto.custom_response_matcher.is_some());
    }

    #[test]
    fn custom_response_config_with_combined_matchers() {
        let config = CustomResponseConfig {
            matchers: vec![
                ResponseMatcherRule {
                    status_code: StatusCodeMatcher::Exact { code: 429 },
                    response: LocalResponsePolicy {
                        status_code: Some(429),
                        body: Some(
                            "{\"error\": \"Rate limit exceeded\", \"retry_after\": 60}".to_string(),
                        ),
                        headers: {
                            let mut h = HashMap::new();
                            h.insert("content-type".to_string(), "application/json".to_string());
                            h.insert("retry-after".to_string(), "60".to_string());
                            h
                        },
                    },
                },
                ResponseMatcherRule {
                    status_code: StatusCodeMatcher::Range { min: 500, max: 599 },
                    response: LocalResponsePolicy::json_error(500, "internal server error"),
                },
                ResponseMatcherRule {
                    status_code: StatusCodeMatcher::List { codes: vec![400, 401, 403] },
                    response: LocalResponsePolicy::json_error(400, "authentication error"),
                },
            ],
            custom_response_matcher: None,
        };

        assert!(config.validate().is_ok());
        let any = config.to_any().expect("to_any should succeed for combined matchers");
        assert_eq!(any.type_url, CUSTOM_RESPONSE_TYPE_URL);
        assert!(!any.value.is_empty());

        // Verify the proto contains a valid matcher structure
        let proto = CustomResponseProto::decode(any.value.as_slice()).expect("decode proto");
        assert!(proto.custom_response_matcher.is_some());

        // Verify we have a matcher list with 3 field matchers
        let matcher = proto.custom_response_matcher.unwrap();
        if let Some(envoy_types::pb::xds::r#type::matcher::v3::matcher::MatcherType::MatcherList(
            list,
        )) = matcher.matcher_type
        {
            assert_eq!(list.matchers.len(), 3, "Should have 3 matchers for exact, range, and list");
        } else {
            panic!("Expected MatcherList matcher type");
        }
    }

    #[test]
    fn custom_response_config_empty_list_fails() {
        let config = CustomResponseConfig {
            matchers: vec![ResponseMatcherRule {
                status_code: StatusCodeMatcher::List { codes: vec![] },
                response: LocalResponsePolicy::json_error(400, "error"),
            }],
            custom_response_matcher: None,
        };

        // Validation should fail for empty list
        assert!(config.validate().is_err());
    }

    #[test]
    fn custom_response_config_single_code_range() {
        // A range with min == max should work (single code)
        let config = CustomResponseConfig {
            matchers: vec![ResponseMatcherRule {
                status_code: StatusCodeMatcher::Range { min: 404, max: 404 },
                response: LocalResponsePolicy::json_error(404, "not found"),
            }],
            custom_response_matcher: None,
        };

        assert!(config.validate().is_ok());
        let any = config.to_any().expect("to_any should succeed for single-code range");
        assert!(!any.value.is_empty());
    }
}
