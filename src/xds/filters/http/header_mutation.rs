//! Header Mutation HTTP filter configuration helpers
//!
//! This module provides configuration for Envoy's header mutation filter,
//! which allows adding, removing, or modifying HTTP headers.

use crate::xds::filters::{any_from_message, invalid_config};
use envoy_types::pb::envoy::config::common::mutation_rules::v3::{
    header_mutation::Action, HeaderMutation,
};
use envoy_types::pb::envoy::config::core::v3::{HeaderValue, HeaderValueOption};
use envoy_types::pb::envoy::extensions::filters::http::header_mutation::v3::{
    HeaderMutation as HeaderMutationProto, HeaderMutationPerRoute as HeaderMutationPerRouteProto,
    Mutations,
};
use envoy_types::pb::google::protobuf::Any as EnvoyAny;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

const HEADER_MUTATION_TYPE_URL: &str =
    "type.googleapis.com/envoy.extensions.filters.http.header_mutation.v3.HeaderMutation";
const HEADER_MUTATION_PER_ROUTE_TYPE_URL: &str =
    "type.googleapis.com/envoy.extensions.filters.http.header_mutation.v3.HeaderMutationPerRoute";

/// Configuration for header mutation filter
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Default)]
pub struct HeaderMutationConfig {
    /// Headers to add/modify in requests
    #[serde(default)]
    pub request_headers_to_add: Vec<HeaderMutationEntry>,
    /// Headers to remove from requests
    #[serde(default)]
    pub request_headers_to_remove: Vec<String>,
    /// Headers to add/modify in responses
    #[serde(default)]
    pub response_headers_to_add: Vec<HeaderMutationEntry>,
    /// Headers to remove from responses
    #[serde(default)]
    pub response_headers_to_remove: Vec<String>,
}

impl HeaderMutationConfig {
    /// Validate configuration
    pub fn validate(&self) -> Result<(), crate::Error> {
        for entry in &self.request_headers_to_add {
            entry.validate("request")?;
        }

        for entry in &self.response_headers_to_add {
            entry.validate("response")?;
        }

        for header in &self.request_headers_to_remove {
            if header.trim().is_empty() {
                return Err(invalid_config(
                    "HeaderMutation request_headers_to_remove entries cannot be empty",
                ));
            }
        }

        for header in &self.response_headers_to_remove {
            if header.trim().is_empty() {
                return Err(invalid_config(
                    "HeaderMutation response_headers_to_remove entries cannot be empty",
                ));
            }
        }

        Ok(())
    }

    /// Convert to Envoy Any payload
    pub fn to_any(&self) -> Result<EnvoyAny, crate::Error> {
        self.validate()?;

        let mut request_mutations = Vec::new();

        // Add headers to add
        for entry in &self.request_headers_to_add {
            request_mutations.push(entry.to_append_proto()?);
        }

        // Add headers to remove
        for header_name in &self.request_headers_to_remove {
            request_mutations
                .push(HeaderMutation { action: Some(Action::Remove(header_name.clone())) });
        }

        let mut response_mutations = Vec::new();

        // Add headers to add
        for entry in &self.response_headers_to_add {
            response_mutations.push(entry.to_append_proto()?);
        }

        // Add headers to remove
        for header_name in &self.response_headers_to_remove {
            response_mutations
                .push(HeaderMutation { action: Some(Action::Remove(header_name.clone())) });
        }

        let mutations = Mutations {
            request_mutations,
            response_mutations,
            query_parameter_mutations: Vec::new(),
            response_trailers_mutations: Vec::new(),
            request_trailers_mutations: Vec::new(),
        };

        let proto = HeaderMutationProto {
            mutations: Some(mutations),
            most_specific_header_mutations_wins: false,
        };

        Ok(any_from_message(HEADER_MUTATION_TYPE_URL, &proto))
    }

    /// Build configuration from Envoy proto
    pub fn from_proto(proto: &HeaderMutationProto) -> Result<Self, crate::Error> {
        let default_mutations = Mutations {
            request_mutations: Vec::new(),
            response_mutations: Vec::new(),
            query_parameter_mutations: Vec::new(),
            response_trailers_mutations: Vec::new(),
            request_trailers_mutations: Vec::new(),
        };
        let mutations = proto.mutations.as_ref().unwrap_or(&default_mutations);

        let mut request_headers_to_add = Vec::new();
        let mut request_headers_to_remove = Vec::new();

        for mutation in &mutations.request_mutations {
            match &mutation.action {
                Some(Action::Append(hvo)) => {
                    if let Ok(entry) = HeaderMutationEntry::from_header_value_option(hvo) {
                        request_headers_to_add.push(entry);
                    }
                }
                Some(Action::Remove(name)) => {
                    request_headers_to_remove.push(name.clone());
                }
                None => {}
            }
        }

        let mut response_headers_to_add = Vec::new();
        let mut response_headers_to_remove = Vec::new();

        for mutation in &mutations.response_mutations {
            match &mutation.action {
                Some(Action::Append(hvo)) => {
                    if let Ok(entry) = HeaderMutationEntry::from_header_value_option(hvo) {
                        response_headers_to_add.push(entry);
                    }
                }
                Some(Action::Remove(name)) => {
                    response_headers_to_remove.push(name.clone());
                }
                None => {}
            }
        }

        let config = Self {
            request_headers_to_add,
            request_headers_to_remove,
            response_headers_to_add,
            response_headers_to_remove,
        };

        config.validate()?;
        Ok(config)
    }
}

/// Single header mutation entry
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct HeaderMutationEntry {
    /// Header name
    pub key: String,
    /// Header value
    pub value: String,
    /// Whether to append if header already exists (default: false = overwrite)
    #[serde(default)]
    pub append: bool,
}

impl HeaderMutationEntry {
    fn validate(&self, context: &str) -> Result<(), crate::Error> {
        if self.key.trim().is_empty() {
            return Err(invalid_config(format!(
                "HeaderMutation {context} header key cannot be empty"
            )));
        }
        Ok(())
    }

    fn to_append_proto(&self) -> Result<HeaderMutation, crate::Error> {
        self.validate("entry")?;

        Ok(HeaderMutation {
            action: Some(Action::Append(HeaderValueOption {
                header: Some(HeaderValue {
                    key: self.key.clone(),
                    value: self.value.clone(),
                    raw_value: Vec::new(),
                }),
                #[allow(deprecated)]
                append: None, // Deprecated field, use append_action instead
                append_action: if self.append {
                    1 // APPEND_IF_EXISTS_OR_ADD
                } else {
                    0 // OVERWRITE_IF_EXISTS_OR_ADD
                },
                keep_empty_value: false,
            })),
        })
    }

    fn from_header_value_option(proto: &HeaderValueOption) -> Result<Self, crate::Error> {
        let header = proto
            .header
            .as_ref()
            .ok_or_else(|| invalid_config("HeaderValueOption requires header field"))?;

        Ok(Self {
            key: header.key.clone(),
            value: header.value.clone(),
            append: proto.append_action == 1,
        })
    }
}

/// Per-route header mutation configuration
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct HeaderMutationPerRouteConfig {
    /// Headers to add/modify in requests
    #[serde(default)]
    pub request_headers_to_add: Vec<HeaderMutationEntry>,
    /// Headers to remove from requests
    #[serde(default)]
    pub request_headers_to_remove: Vec<String>,
    /// Headers to add/modify in responses
    #[serde(default)]
    pub response_headers_to_add: Vec<HeaderMutationEntry>,
    /// Headers to remove from responses
    #[serde(default)]
    pub response_headers_to_remove: Vec<String>,
}

impl HeaderMutationPerRouteConfig {
    /// Validate configuration
    pub fn validate(&self) -> Result<(), crate::Error> {
        for entry in &self.request_headers_to_add {
            entry.validate("request")?;
        }

        for entry in &self.response_headers_to_add {
            entry.validate("response")?;
        }

        for header in &self.request_headers_to_remove {
            if header.trim().is_empty() {
                return Err(invalid_config(
                    "HeaderMutationPerRoute request_headers_to_remove entries cannot be empty",
                ));
            }
        }

        for header in &self.response_headers_to_remove {
            if header.trim().is_empty() {
                return Err(invalid_config(
                    "HeaderMutationPerRoute response_headers_to_remove entries cannot be empty",
                ));
            }
        }

        Ok(())
    }

    /// Convert to Envoy Any payload
    pub fn to_any(&self) -> Result<EnvoyAny, crate::Error> {
        self.validate()?;

        let mut request_mutations = Vec::new();

        // Add headers to add
        for entry in &self.request_headers_to_add {
            request_mutations.push(entry.to_append_proto()?);
        }

        // Add headers to remove
        for header_name in &self.request_headers_to_remove {
            request_mutations
                .push(HeaderMutation { action: Some(Action::Remove(header_name.clone())) });
        }

        let mut response_mutations = Vec::new();

        // Add headers to add
        for entry in &self.response_headers_to_add {
            response_mutations.push(entry.to_append_proto()?);
        }

        // Add headers to remove
        for header_name in &self.response_headers_to_remove {
            response_mutations
                .push(HeaderMutation { action: Some(Action::Remove(header_name.clone())) });
        }

        let mutations = Mutations {
            request_mutations,
            response_mutations,
            query_parameter_mutations: Vec::new(),
            response_trailers_mutations: Vec::new(),
            request_trailers_mutations: Vec::new(),
        };

        let proto = HeaderMutationPerRouteProto { mutations: Some(mutations) };

        Ok(any_from_message(HEADER_MUTATION_PER_ROUTE_TYPE_URL, &proto))
    }

    /// Build configuration from Envoy proto
    pub fn from_proto(proto: &HeaderMutationPerRouteProto) -> Result<Self, crate::Error> {
        let default_mutations = Mutations {
            request_mutations: Vec::new(),
            response_mutations: Vec::new(),
            query_parameter_mutations: Vec::new(),
            response_trailers_mutations: Vec::new(),
            request_trailers_mutations: Vec::new(),
        };
        let mutations = proto.mutations.as_ref().unwrap_or(&default_mutations);

        let mut request_headers_to_add = Vec::new();
        let mut request_headers_to_remove = Vec::new();

        for mutation in &mutations.request_mutations {
            match &mutation.action {
                Some(Action::Append(hvo)) => {
                    if let Ok(entry) = HeaderMutationEntry::from_header_value_option(hvo) {
                        request_headers_to_add.push(entry);
                    }
                }
                Some(Action::Remove(name)) => {
                    request_headers_to_remove.push(name.clone());
                }
                None => {}
            }
        }

        let mut response_headers_to_add = Vec::new();
        let mut response_headers_to_remove = Vec::new();

        for mutation in &mutations.response_mutations {
            match &mutation.action {
                Some(Action::Append(hvo)) => {
                    if let Ok(entry) = HeaderMutationEntry::from_header_value_option(hvo) {
                        response_headers_to_add.push(entry);
                    }
                }
                Some(Action::Remove(name)) => {
                    response_headers_to_remove.push(name.clone());
                }
                None => {}
            }
        }

        let config = Self {
            request_headers_to_add,
            request_headers_to_remove,
            response_headers_to_add,
            response_headers_to_remove,
        };

        config.validate()?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;

    fn sample_config() -> HeaderMutationConfig {
        HeaderMutationConfig {
            request_headers_to_add: vec![
                HeaderMutationEntry {
                    key: "x-custom-header".into(),
                    value: "custom-value".into(),
                    append: false,
                },
                HeaderMutationEntry {
                    key: "x-append-header".into(),
                    value: "append-value".into(),
                    append: true,
                },
            ],
            request_headers_to_remove: vec!["x-remove-me".into()],
            response_headers_to_add: vec![HeaderMutationEntry {
                key: "x-response-header".into(),
                value: "response-value".into(),
                append: false,
            }],
            response_headers_to_remove: vec!["server".into()],
        }
    }

    #[test]
    fn validates_empty_header_key() {
        let mut config = sample_config();
        config.request_headers_to_add[0].key = "".into();
        let err = config.validate().expect_err("empty key should fail");
        assert!(format!("{err}").contains("key"));
    }

    #[test]
    fn validates_empty_remove_entry() {
        let mut config = sample_config();
        config.request_headers_to_remove.push("".into());
        let err = config.validate().expect_err("empty remove should fail");
        assert!(format!("{err}").contains("cannot be empty"));
    }

    #[test]
    fn builds_proto() {
        let config = sample_config();
        let any = config.to_any().expect("to_any");
        assert_eq!(any.type_url, HEADER_MUTATION_TYPE_URL);
        assert!(!any.value.is_empty());
    }

    #[test]
    fn proto_round_trip() {
        let config = sample_config();
        let any = config.to_any().expect("to_any");

        let proto = HeaderMutationProto::decode(any.value.as_slice()).expect("decode proto");
        let round_tripped = HeaderMutationConfig::from_proto(&proto).expect("from_proto");

        assert_eq!(round_tripped.request_headers_to_add.len(), 2);
        assert_eq!(round_tripped.request_headers_to_add[0].key, "x-custom-header");
        assert_eq!(round_tripped.request_headers_to_add[0].value, "custom-value");
        assert!(!round_tripped.request_headers_to_add[0].append);
        assert_eq!(round_tripped.request_headers_to_add[1].key, "x-append-header");
        assert!(round_tripped.request_headers_to_add[1].append);
    }

    #[test]
    fn handles_empty_config() {
        let config = HeaderMutationConfig::default();
        let any = config.to_any().expect("to_any");
        assert!(!any.value.is_empty());
    }

    #[test]
    fn append_action_mapping() {
        let entry = HeaderMutationEntry { key: "test".into(), value: "value".into(), append: true };
        let proto = entry.to_append_proto().expect("to_append_proto");
        match proto.action {
            Some(Action::Append(hvo)) => assert_eq!(hvo.append_action, 1),
            _ => panic!("Expected Append action"),
        }

        let entry =
            HeaderMutationEntry { key: "test".into(), value: "value".into(), append: false };
        let proto = entry.to_append_proto().expect("to_append_proto");
        match proto.action {
            Some(Action::Append(hvo)) => assert_eq!(hvo.append_action, 0),
            _ => panic!("Expected Append action"),
        }
    }

    #[test]
    fn per_route_builds_proto() {
        let config = HeaderMutationPerRouteConfig {
            request_headers_to_add: vec![HeaderMutationEntry {
                key: "x-route-header".into(),
                value: "route-value".into(),
                append: false,
            }],
            request_headers_to_remove: vec!["x-remove-route".into()],
            response_headers_to_add: Vec::new(),
            response_headers_to_remove: Vec::new(),
        };

        let any = config.to_any().expect("to_any");
        assert_eq!(any.type_url, HEADER_MUTATION_PER_ROUTE_TYPE_URL);
        assert!(!any.value.is_empty());
    }

    #[test]
    fn per_route_proto_round_trip() {
        let config = HeaderMutationPerRouteConfig {
            request_headers_to_add: vec![HeaderMutationEntry {
                key: "x-custom".into(),
                value: "custom".into(),
                append: true,
            }],
            request_headers_to_remove: vec!["x-remove".into()],
            response_headers_to_add: vec![HeaderMutationEntry {
                key: "x-response".into(),
                value: "resp".into(),
                append: false,
            }],
            response_headers_to_remove: vec!["server".into()],
        };

        let any = config.to_any().expect("to_any");
        let proto = HeaderMutationPerRouteProto::decode(any.value.as_slice()).expect("decode");
        let round_tripped = HeaderMutationPerRouteConfig::from_proto(&proto).expect("from_proto");

        assert_eq!(round_tripped.request_headers_to_add.len(), 1);
        assert_eq!(round_tripped.request_headers_to_add[0].key, "x-custom");
        assert!(round_tripped.request_headers_to_add[0].append);
        assert_eq!(round_tripped.request_headers_to_remove, vec!["x-remove"]);
        assert_eq!(round_tripped.response_headers_to_add.len(), 1);
        assert_eq!(round_tripped.response_headers_to_remove, vec!["server"]);
    }

    #[test]
    fn per_route_validates_empty_key() {
        let config = HeaderMutationPerRouteConfig {
            request_headers_to_add: vec![HeaderMutationEntry {
                key: "".into(),
                value: "value".into(),
                append: false,
            }],
            request_headers_to_remove: Vec::new(),
            response_headers_to_add: Vec::new(),
            response_headers_to_remove: Vec::new(),
        };

        let err = config.validate().expect_err("empty key should fail");
        assert!(format!("{err}").contains("key"));
    }

    #[test]
    fn per_route_validates_empty_remove_entry() {
        let config = HeaderMutationPerRouteConfig {
            request_headers_to_add: Vec::new(),
            request_headers_to_remove: vec!["".into()],
            response_headers_to_add: Vec::new(),
            response_headers_to_remove: Vec::new(),
        };

        let err = config.validate().expect_err("empty remove should fail");
        assert!(format!("{err}").contains("cannot be empty"));
    }
}
