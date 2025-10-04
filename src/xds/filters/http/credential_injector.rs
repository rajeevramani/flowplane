//! Credential Injector HTTP filter configuration helpers
//!
//! This module provides configuration for Envoy's credential injector filter,
//! which injects credentials into outgoing HTTP requests for workload authentication.

use crate::xds::filters::{any_from_message, invalid_config, Base64Bytes, TypedConfig};
use envoy_types::pb::envoy::config::core::v3::TypedExtensionConfig;
use envoy_types::pb::envoy::extensions::filters::http::credential_injector::v3::CredentialInjector as CredentialInjectorProto;
use envoy_types::pb::google::protobuf::Any as EnvoyAny;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

const CREDENTIAL_INJECTOR_TYPE_URL: &str =
    "type.googleapis.com/envoy.extensions.filters.http.credential_injector.v3.CredentialInjector";

/// Configuration for credential injector filter
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CredentialInjectorConfig {
    /// Whether to overwrite existing authorization headers
    #[serde(default)]
    pub overwrite: bool,
    /// Whether to allow requests without credentials (default: false, returns 401)
    #[serde(default)]
    pub allow_request_without_credential: bool,
    /// Credential configuration (name and typed config)
    pub credential: Option<CredentialConfig>,
}

/// Credential configuration for injection
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CredentialConfig {
    /// Name identifier for the credential extension
    pub name: String,
    /// Typed configuration for the credential extension
    #[serde(flatten)]
    pub config: TypedConfig,
}

impl CredentialInjectorConfig {
    /// Validate configuration
    pub fn validate(&self) -> Result<(), crate::Error> {
        if let Some(ref cred) = self.credential {
            if cred.name.trim().is_empty() {
                return Err(invalid_config("CredentialInjector credential name cannot be empty"));
            }
        }
        Ok(())
    }

    /// Convert to Envoy Any payload
    pub fn to_any(&self) -> Result<EnvoyAny, crate::Error> {
        self.validate()?;

        let credential = self.credential.as_ref().map(|cred| TypedExtensionConfig {
            name: cred.name.clone(),
            typed_config: Some(cred.config.to_any()),
        });

        let proto = CredentialInjectorProto {
            overwrite: self.overwrite,
            allow_request_without_credential: self.allow_request_without_credential,
            credential,
        };

        Ok(any_from_message(CREDENTIAL_INJECTOR_TYPE_URL, &proto))
    }

    /// Build configuration from Envoy proto
    pub fn from_proto(proto: &CredentialInjectorProto) -> Result<Self, crate::Error> {
        let credential = proto.credential.as_ref().map(|cred| {
            let typed_config = cred.typed_config.as_ref().map(|any| TypedConfig {
                type_url: any.type_url.clone(),
                value: Base64Bytes(any.value.clone()),
            });

            CredentialConfig {
                name: cred.name.clone(),
                config: typed_config.unwrap_or_else(|| TypedConfig {
                    type_url: String::new(),
                    value: Base64Bytes(Vec::new()),
                }),
            }
        });

        let config = Self {
            overwrite: proto.overwrite,
            allow_request_without_credential: proto.allow_request_without_credential,
            credential,
        };

        config.validate()?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;

    fn sample_config() -> CredentialInjectorConfig {
        CredentialInjectorConfig {
            overwrite: true,
            allow_request_without_credential: false,
            credential: Some(CredentialConfig {
                name: "oauth2_credential".into(),
                config: TypedConfig {
                    type_url: "type.googleapis.com/envoy.extensions.http.injected_credentials.oauth2.v3.OAuth2".into(),
                    value: Base64Bytes(vec![1, 2, 3, 4]),
                },
            }),
        }
    }

    #[test]
    fn validates_credential_name() {
        let mut config = sample_config();
        config.credential = Some(CredentialConfig {
            name: "".into(),
            config: TypedConfig { type_url: "test".into(), value: Base64Bytes(vec![]) },
        });

        let err = config.validate().expect_err("empty name should fail");
        assert!(format!("{err}").contains("name cannot be empty"));
    }

    #[test]
    fn builds_proto() {
        let config = sample_config();
        let any = config.to_any().expect("to_any");
        assert_eq!(any.type_url, CREDENTIAL_INJECTOR_TYPE_URL);
        assert!(!any.value.is_empty());
    }

    #[test]
    fn proto_round_trip() {
        let config = sample_config();
        let any = config.to_any().expect("to_any");

        let proto = CredentialInjectorProto::decode(any.value.as_slice()).expect("decode proto");
        let round_tripped = CredentialInjectorConfig::from_proto(&proto).expect("from_proto");

        assert!(round_tripped.overwrite);
        assert!(!round_tripped.allow_request_without_credential);
        assert!(round_tripped.credential.is_some());

        let cred = round_tripped.credential.unwrap();
        assert_eq!(cred.name, "oauth2_credential");
        assert_eq!(
            cred.config.type_url,
            "type.googleapis.com/envoy.extensions.http.injected_credentials.oauth2.v3.OAuth2"
        );
    }

    #[test]
    fn handles_no_credential() {
        let config = CredentialInjectorConfig {
            overwrite: false,
            allow_request_without_credential: true,
            credential: None,
        };

        let any = config.to_any().expect("to_any");
        assert!(!any.value.is_empty());
    }

    #[test]
    fn default_flags() {
        let config = CredentialInjectorConfig {
            overwrite: false,
            allow_request_without_credential: false,
            credential: None,
        };

        assert!(!config.overwrite);
        assert!(!config.allow_request_without_credential);
    }
}
