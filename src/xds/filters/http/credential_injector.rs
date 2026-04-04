//! Credential Injector HTTP filter configuration helpers
//!
//! This module provides configuration for Envoy's credential injector filter,
//! which injects credentials into outgoing HTTP requests for workload authentication.
//! Credentials can be specified inline via `credential` or fetched from the secrets
//! system via SDS using `secret_ref`.

use crate::xds::filters::{
    any_from_message, build_sds_secret_config, invalid_config, Base64Bytes, TypedConfig,
};
use envoy_types::pb::envoy::config::core::v3::TypedExtensionConfig;
use envoy_types::pb::envoy::extensions::filters::http::credential_injector::v3::CredentialInjector as CredentialInjectorProto;
use envoy_types::pb::envoy::extensions::http::injected_credentials::generic::v3::Generic as GenericCredential;
use envoy_types::pb::google::protobuf::Any as EnvoyAny;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

const CREDENTIAL_INJECTOR_TYPE_URL: &str =
    "type.googleapis.com/envoy.extensions.filters.http.credential_injector.v3.CredentialInjector";

const GENERIC_CREDENTIAL_TYPE_URL: &str =
    "type.googleapis.com/envoy.extensions.http.injected_credentials.generic.v3.Generic";

/// Configuration for credential injector filter
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CredentialInjectorConfig {
    /// Whether to overwrite existing authorization headers
    #[serde(default)]
    pub overwrite: bool,
    /// Whether to allow requests without credentials (default: false, returns 401)
    #[serde(default)]
    pub allow_request_without_credential: bool,
    /// Credential configuration (name and typed config) for inline credential specification
    pub credential: Option<CredentialConfig>,
    /// Optional reference to an SDS-managed secret for credential injection.
    /// When set, the credential value is fetched via SDS instead of inline config.
    /// Takes precedence over `credential` when both are provided.
    #[serde(default)]
    pub secret_ref: Option<SecretRef>,
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

/// Reference to an SDS-managed secret for credential injection.
///
/// When provided on a credential injector filter, the credential value is fetched
/// via Envoy's Secret Discovery Service (SDS) using a Generic credential type,
/// instead of being specified inline.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SecretRef {
    /// Name of the secret in the secrets system (used as the SDS secret name)
    pub name: String,
    /// HTTP header to inject the credential into (e.g., "Authorization", "X-Api-Key").
    /// Defaults to "Authorization" if not specified.
    #[serde(default = "default_header")]
    pub header: String,
    /// Optional prefix prepended to the credential value before injection.
    /// For example, "Bearer " for bearer tokens or "Basic " for basic auth.
    #[serde(default)]
    pub header_prefix: Option<String>,
}

fn default_header() -> String {
    "Authorization".to_string()
}

impl CredentialInjectorConfig {
    /// Validate configuration
    pub fn validate(&self) -> Result<(), crate::Error> {
        if let Some(ref cred) = self.credential {
            if cred.name.trim().is_empty() {
                return Err(invalid_config("CredentialInjector credential name cannot be empty"));
            }
        }
        if let Some(ref secret) = self.secret_ref {
            if secret.name.trim().is_empty() {
                return Err(invalid_config("CredentialInjector secret_ref.name cannot be empty"));
            }
            if secret.header.trim().is_empty() {
                return Err(invalid_config("CredentialInjector secret_ref.header cannot be empty"));
            }
        }
        Ok(())
    }

    /// Convert to Envoy Any payload
    pub fn to_any(&self) -> Result<EnvoyAny, crate::Error> {
        self.validate()?;

        // secret_ref takes precedence over inline credential
        let credential = if let Some(ref secret) = self.secret_ref {
            let generic = GenericCredential {
                credential: Some(build_sds_secret_config(&secret.name)),
                header: secret.header.clone(),
            };
            Some(TypedExtensionConfig {
                name: "envoy.http.injected_credentials.generic".to_string(),
                typed_config: Some(any_from_message(GENERIC_CREDENTIAL_TYPE_URL, &generic)),
            })
        } else {
            self.credential.as_ref().map(|cred| TypedExtensionConfig {
                name: cred.name.clone(),
                typed_config: Some(cred.config.to_any()),
            })
        };

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
            // SDS secret references are resolved at config time, not stored in proto
            secret_ref: None,
        };

        config.validate()?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use envoy_types::pb::envoy::config::core::v3::config_source::ConfigSourceSpecifier;
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
            secret_ref: None,
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

        let cred = round_tripped.credential.as_ref().expect("credential");
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
            secret_ref: None,
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
            secret_ref: None,
        };

        assert!(!config.overwrite);
        assert!(!config.allow_request_without_credential);
    }

    // --- secret_ref tests ---

    #[test]
    fn secret_ref_builds_generic_sds_credential() {
        let config = CredentialInjectorConfig {
            overwrite: true,
            allow_request_without_credential: false,
            credential: None,
            secret_ref: Some(SecretRef {
                name: "my-api-key-secret".to_string(),
                header: "X-Api-Key".to_string(),
                header_prefix: None,
            }),
        };

        assert!(config.validate().is_ok());
        let any = config.to_any().expect("to_any");
        assert_eq!(any.type_url, CREDENTIAL_INJECTOR_TYPE_URL);

        // Decode the outer CredentialInjector proto
        let proto = CredentialInjectorProto::decode(any.value.as_slice()).expect("decode proto");
        let cred = proto.credential.expect("should have credential");
        assert_eq!(cred.name, "envoy.http.injected_credentials.generic");

        // Decode the inner Generic credential
        let tc = cred.typed_config.expect("typed_config");
        assert_eq!(tc.type_url, GENERIC_CREDENTIAL_TYPE_URL);
        let generic = GenericCredential::decode(tc.value.as_slice()).expect("decode generic");
        assert_eq!(generic.header, "X-Api-Key");

        // Verify SDS config inside Generic
        let sds = generic.credential.expect("sds config");
        assert_eq!(sds.name, "my-api-key-secret");
        let source = sds.sds_config.expect("sds_config source");
        assert!(matches!(source.config_source_specifier, Some(ConfigSourceSpecifier::Ads(_))));
    }

    #[test]
    fn secret_ref_with_bearer_prefix() {
        let config = CredentialInjectorConfig {
            overwrite: false,
            allow_request_without_credential: false,
            credential: None,
            secret_ref: Some(SecretRef {
                name: "oauth-token".to_string(),
                header: "Authorization".to_string(),
                header_prefix: Some("Bearer ".to_string()),
            }),
        };

        let any = config.to_any().expect("to_any");
        let proto = CredentialInjectorProto::decode(any.value.as_slice()).expect("decode");
        let tc = proto.credential.expect("cred").typed_config.expect("tc");
        let generic = GenericCredential::decode(tc.value.as_slice()).expect("generic");
        assert_eq!(generic.header, "Authorization");
        assert_eq!(generic.credential.expect("sds").name, "oauth-token");
    }

    #[test]
    fn secret_ref_takes_precedence_over_inline() {
        let config = CredentialInjectorConfig {
            overwrite: false,
            allow_request_without_credential: false,
            credential: Some(CredentialConfig {
                name: "inline-cred".into(),
                config: TypedConfig { type_url: "test".into(), value: Base64Bytes(vec![1]) },
            }),
            secret_ref: Some(SecretRef {
                name: "sds-secret".to_string(),
                header: "X-Api-Key".to_string(),
                header_prefix: None,
            }),
        };

        let any = config.to_any().expect("to_any");
        let proto = CredentialInjectorProto::decode(any.value.as_slice()).expect("decode");
        let cred = proto.credential.expect("should have credential");
        // Should use generic credential via SDS, not inline
        assert_eq!(cred.name, "envoy.http.injected_credentials.generic");
        let tc = cred.typed_config.expect("tc");
        assert_eq!(tc.type_url, GENERIC_CREDENTIAL_TYPE_URL);
    }

    #[test]
    fn empty_secret_ref_name_fails_validation() {
        let config = CredentialInjectorConfig {
            overwrite: false,
            allow_request_without_credential: false,
            credential: None,
            secret_ref: Some(SecretRef {
                name: String::new(),
                header: "Authorization".to_string(),
                header_prefix: None,
            }),
        };

        let err = config.validate().unwrap_err();
        assert!(format!("{err}").contains("secret_ref.name"));
    }

    #[test]
    fn whitespace_secret_ref_name_fails_validation() {
        let config = CredentialInjectorConfig {
            overwrite: false,
            allow_request_without_credential: false,
            credential: None,
            secret_ref: Some(SecretRef {
                name: "   ".to_string(),
                header: "Authorization".to_string(),
                header_prefix: None,
            }),
        };

        let err = config.validate().unwrap_err();
        assert!(format!("{err}").contains("secret_ref.name"));
    }

    #[test]
    fn empty_secret_ref_header_fails_validation() {
        let config = CredentialInjectorConfig {
            overwrite: false,
            allow_request_without_credential: false,
            credential: None,
            secret_ref: Some(SecretRef {
                name: "my-secret".to_string(),
                header: String::new(),
                header_prefix: None,
            }),
        };

        let err = config.validate().unwrap_err();
        assert!(format!("{err}").contains("secret_ref.header"));
    }

    #[test]
    fn from_proto_sets_secret_ref_to_none() {
        // SDS refs are config-time only; from_proto always returns None for secret_ref
        let config = CredentialInjectorConfig {
            overwrite: true,
            allow_request_without_credential: false,
            credential: None,
            secret_ref: Some(SecretRef {
                name: "will-be-resolved".to_string(),
                header: "Authorization".to_string(),
                header_prefix: None,
            }),
        };

        let any = config.to_any().expect("to_any");
        let proto = CredentialInjectorProto::decode(any.value.as_slice()).expect("decode");
        let restored = CredentialInjectorConfig::from_proto(&proto).expect("from_proto");
        assert!(restored.secret_ref.is_none());
    }

    #[test]
    fn secret_ref_deserialize_from_json() {
        let json = r#"{
            "overwrite": true,
            "secret_ref": {
                "name": "upstream-api-key",
                "header": "X-Api-Key"
            }
        }"#;

        let config: CredentialInjectorConfig = serde_json::from_str(json).expect("deserialize");
        assert!(config.validate().is_ok());
        let sr = config.secret_ref.expect("should have secret_ref");
        assert_eq!(sr.name, "upstream-api-key");
        assert_eq!(sr.header, "X-Api-Key");
        assert!(sr.header_prefix.is_none());
    }

    #[test]
    fn secret_ref_with_prefix_deserialize_from_json() {
        let json = r#"{
            "secret_ref": {
                "name": "my-bearer-token",
                "header": "Authorization",
                "header_prefix": "Bearer "
            }
        }"#;

        let config: CredentialInjectorConfig = serde_json::from_str(json).expect("deserialize");
        let sr = config.secret_ref.expect("should have secret_ref");
        assert_eq!(sr.name, "my-bearer-token");
        assert_eq!(sr.header_prefix.as_deref(), Some("Bearer "));
    }

    #[test]
    fn secret_ref_default_header_is_authorization() {
        let json = r#"{
            "secret_ref": {
                "name": "my-secret"
            }
        }"#;

        let config: CredentialInjectorConfig = serde_json::from_str(json).expect("deserialize");
        let sr = config.secret_ref.expect("should have secret_ref");
        assert_eq!(sr.header, "Authorization");
    }

    #[test]
    fn secret_ref_optional_in_json() {
        let json = r#"{
            "overwrite": false,
            "credential": null
        }"#;

        let config: CredentialInjectorConfig = serde_json::from_str(json).expect("deserialize");
        assert!(config.secret_ref.is_none());
        assert!(config.credential.is_none());
    }
}
