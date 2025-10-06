//! Custom Response HTTP filter configuration helpers
//!
//! This module provides configuration for Envoy's custom response filter,
//! which allows defining custom response policies based on matcher trees.

use crate::xds::filters::{any_from_message, invalid_config, Base64Bytes, TypedConfig};
use envoy_types::pb::envoy::extensions::filters::http::custom_response::v3::CustomResponse as CustomResponseProto;
use envoy_types::pb::google::protobuf::Any as EnvoyAny;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use utoipa::ToSchema;

const CUSTOM_RESPONSE_TYPE_URL: &str =
    "type.googleapis.com/envoy.extensions.filters.http.custom_response.v3.CustomResponse";

const CUSTOM_RESPONSE_PER_ROUTE_TYPE_URL: &str =
    "type.googleapis.com/envoy.config.route.v3.FilterConfig";

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

        Self {
            status_code: Some(status_code),
            body: Some(body),
            headers,
        }
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
    fn build_envoy_matcher(&self) -> Result<envoy_types::pb::xds::r#type::matcher::v3::Matcher, crate::Error> {
        use envoy_types::pb::envoy::config::core::v3::HeaderValueOption;
        use envoy_types::pb::envoy::extensions::http::custom_response::local_response_policy::v3::LocalResponsePolicy as EnvoyLocalResponsePolicy;
        use envoy_types::pb::google::protobuf::UInt32Value;
        use envoy_types::pb::xds::core::v3::TypedExtensionConfig;
        use envoy_types::pb::xds::r#type::matcher::v3::matcher::OnMatch;
        use envoy_types::pb::xds::r#type::matcher::v3::{
            matcher, Matcher, matcher::MatcherList, matcher::matcher_list::FieldMatcher,
            matcher::matcher_list::Predicate, matcher::matcher_list::predicate::SinglePredicate,
            matcher::matcher_list::predicate::single_predicate::Matcher as PredicateMatcher,
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
                    HeaderValueOption {
                        header: Some(envoy_types::pb::envoy::config::core::v3::HeaderValue {
                            key: k.clone(),
                            value: v.clone(),
                            raw_value: vec![],
                        }),
                        append: None, // Deprecated field, use append_action instead
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
                StatusCodeMatcher::Exact { code } => {
                    Predicate {
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
                // TODO: Add Range and List support in future iterations
                _ => {
                    return Err(invalid_config(
                        "Only Exact status code matching is currently supported. Range and List support coming soon."
                    ));
                }
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
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Default)]
pub struct CustomResponsePerRouteConfig {
    /// Whether to disable custom response for this route
    #[serde(default)]
    pub disabled: bool,
}

impl CustomResponsePerRouteConfig {
    /// Validate per-route configuration
    pub fn validate(&self) -> Result<(), crate::Error> {
        // Simple boolean flag, always valid
        Ok(())
    }

    /// Convert to Envoy Any payload for typed_per_filter_config
    pub fn to_any(&self) -> Result<EnvoyAny, crate::Error> {
        use envoy_types::pb::envoy::config::route::v3::FilterConfig;

        self.validate()?;

        let proto = FilterConfig {
            disabled: self.disabled,
            is_optional: false,
            config: None, // Custom response doesn't support per-route config, only disable
        };

        Ok(any_from_message(CUSTOM_RESPONSE_PER_ROUTE_TYPE_URL, &proto))
    }

    /// Build configuration from Envoy proto
    pub fn from_proto(proto: &envoy_types::pb::envoy::config::route::v3::FilterConfig) -> Result<Self, crate::Error> {
        let config = Self {
            disabled: proto.disabled,
        };

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
        assert!(config.validate().is_ok());
    }

    #[test]
    fn per_route_config_disabled() {
        let config = CustomResponsePerRouteConfig { disabled: true };
        assert!(config.validate().is_ok());

        let any = config.to_any().expect("to_any");
        assert_eq!(any.type_url, CUSTOM_RESPONSE_PER_ROUTE_TYPE_URL);
    }

    #[test]
    fn per_route_proto_round_trip() {
        use envoy_types::pb::envoy::config::route::v3::FilterConfig;

        let config = CustomResponsePerRouteConfig { disabled: true };
        let any = config.to_any().expect("to_any");

        let proto = FilterConfig::decode(any.value.as_slice()).expect("decode proto");
        assert!(proto.disabled);

        let round_tripped = CustomResponsePerRouteConfig::from_proto(&proto).expect("from_proto");
        assert!(round_tripped.disabled);
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

        let invalid = LocalResponsePolicy {
            status_code: Some(600),
            body: None,
            headers: HashMap::new(),
        };
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
            matchers: vec![
                ResponseMatcherRule {
                    status_code: StatusCodeMatcher::Exact { code: 400 },
                    response: LocalResponsePolicy::json_error(400, "bad request"),
                },
            ],
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
            matchers: vec![
                ResponseMatcherRule {
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
                },
            ],
            custom_response_matcher: None,
        };

        let json = serde_json::to_string(&config).expect("serialize");
        let round_tripped: CustomResponseConfig = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(round_tripped.matchers.len(), 1);
        assert_eq!(round_tripped.matchers[0].status_code, StatusCodeMatcher::Exact { code: 400 });
    }
}
