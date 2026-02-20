//! Secret resource builder for SDS (Secret Discovery Service)
//!
//! Converts database SecretData to Envoy Secret protobuf resources for
//! delivery via the xDS Aggregated Discovery Service.

use crate::domain::{
    CertificateValidationContextSpec, GenericSecretSpec, SecretSpec, StringMatcher,
    TlsCertificateSpec,
};
use crate::storage::SecretData;
use crate::xds::resources::BuiltResource;
use crate::{Error, Result};
use base64::Engine;
use envoy_types::pb::envoy::config::core::v3::{data_source::Specifier, DataSource};
use envoy_types::pb::envoy::extensions::transport_sockets::tls::v3::{
    secret, CertificateValidationContext, GenericSecret, Secret, TlsCertificate,
    TlsSessionTicketKeys,
};
use envoy_types::pb::envoy::r#type::matcher::v3::{
    string_matcher::MatchPattern, StringMatcher as EnvoyStringMatcher,
};
use envoy_types::pb::google::protobuf::Any;
use prost::Message;
use tracing::{debug, warn};

/// Type URL for Envoy Secret resources
pub const SECRET_TYPE_URL: &str =
    "type.googleapis.com/envoy.extensions.transport_sockets.tls.v3.Secret";

/// Build Envoy Secret resources from database entries
pub fn secrets_from_database_entries(
    entries: Vec<SecretData>,
    context: &str,
) -> Result<Vec<BuiltResource>> {
    let mut resources = Vec::with_capacity(entries.len());
    let mut total_bytes = 0;

    for entry in entries {
        match build_secret(&entry) {
            Ok(secret) => {
                let encoded = secret.encode_to_vec();
                debug!(
                    phase = context,
                    secret_name = %entry.name,
                    secret_type = %entry.secret_type,
                    version = entry.version,
                    encoded_size = encoded.len(),
                    "Built secret resource from database entry"
                );

                total_bytes += encoded.len();
                resources.push(BuiltResource {
                    name: entry.name.clone(),
                    resource: Any { type_url: SECRET_TYPE_URL.to_string(), value: encoded },
                });
            }
            Err(e) => {
                warn!(
                    phase = context,
                    secret_name = %entry.name,
                    error = %e,
                    "Failed to build secret resource, skipping"
                );
            }
        }
    }

    if !resources.is_empty() {
        debug!(
            phase = context,
            secret_count = resources.len(),
            total_bytes,
            "Built secret resources from database"
        );
    }

    Ok(resources)
}

/// Build a single Envoy Secret from database entry
fn build_secret(data: &SecretData) -> Result<Secret> {
    // Parse the decrypted configuration JSON
    let spec: SecretSpec = serde_json::from_str(&data.configuration).map_err(|e| {
        Error::config(format!("Invalid secret configuration JSON for '{}': {}", data.name, e))
    })?;

    // Verify the type matches
    if spec.secret_type() != data.secret_type {
        return Err(Error::config(format!(
            "Secret type mismatch for '{}': database says {:?}, config says {:?}",
            data.name,
            data.secret_type,
            spec.secret_type()
        )));
    }

    let secret_type = match spec {
        SecretSpec::GenericSecret(gs) => build_generic_secret(gs)?,
        SecretSpec::TlsCertificate(tc) => build_tls_certificate(tc)?,
        SecretSpec::CertificateValidationContext(cvc) => build_validation_context(cvc)?,
        SecretSpec::SessionTicketKeys { keys } => build_session_ticket_keys(keys)?,
    };

    Ok(Secret { name: data.name.clone(), r#type: Some(secret_type) })
}

/// Build a generic secret (OAuth2 tokens, API keys, HMAC secrets)
fn build_generic_secret(spec: GenericSecretSpec) -> Result<secret::Type> {
    // Decode the base64-encoded secret
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(&spec.secret)
        .map_err(|e| Error::config(format!("Invalid base64 in generic secret: {}", e)))?;

    Ok(secret::Type::GenericSecret(GenericSecret {
        secret: Some(DataSource {
            specifier: Some(Specifier::InlineBytes(decoded)),
            ..Default::default()
        }),
        ..Default::default()
    }))
}

/// Build a TLS certificate with private key
fn build_tls_certificate(spec: TlsCertificateSpec) -> Result<secret::Type> {
    let mut tls_cert = TlsCertificate {
        certificate_chain: Some(DataSource {
            specifier: Some(Specifier::InlineString(spec.certificate_chain)),
            ..Default::default()
        }),
        private_key: Some(DataSource {
            specifier: Some(Specifier::InlineString(spec.private_key)),
            ..Default::default()
        }),
        ..Default::default()
    };

    // Optional password for encrypted private key
    if let Some(password) = spec.password {
        tls_cert.password = Some(DataSource {
            specifier: Some(Specifier::InlineString(password)),
            ..Default::default()
        });
    }

    // Optional OCSP staple response
    if let Some(ocsp_staple) = spec.ocsp_staple {
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&ocsp_staple)
            .map_err(|e| Error::config(format!("Invalid base64 in OCSP staple: {}", e)))?;
        tls_cert.ocsp_staple = Some(DataSource {
            specifier: Some(Specifier::InlineBytes(decoded)),
            ..Default::default()
        });
    }

    Ok(secret::Type::TlsCertificate(tls_cert))
}

/// Build a certificate validation context
fn build_validation_context(spec: CertificateValidationContextSpec) -> Result<secret::Type> {
    let mut ctx = CertificateValidationContext {
        trusted_ca: Some(DataSource {
            specifier: Some(Specifier::InlineString(spec.trusted_ca)),
            ..Default::default()
        }),
        only_verify_leaf_cert_crl: spec.only_verify_leaf_cert_crl,
        ..Default::default()
    };

    // Convert SAN matchers
    #[allow(deprecated)]
    if !spec.match_subject_alt_names.is_empty() {
        ctx.match_subject_alt_names = spec
            .match_subject_alt_names
            .into_iter()
            .map(convert_string_matcher)
            .collect::<Result<Vec<_>>>()?;
    }

    // Optional CRL
    if let Some(crl) = spec.crl {
        ctx.crl = Some(DataSource {
            specifier: Some(Specifier::InlineString(crl)),
            ..Default::default()
        });
    }

    Ok(secret::Type::ValidationContext(ctx))
}

/// Build session ticket keys for TLS resumption
fn build_session_ticket_keys(
    keys: Vec<crate::domain::SessionTicketKeySpec>,
) -> Result<secret::Type> {
    let mut ticket_keys = Vec::new();

    for key_spec in keys {
        let decoded =
            base64::engine::general_purpose::STANDARD.decode(&key_spec.key).map_err(|e| {
                Error::config(format!(
                    "Invalid base64 in session ticket key '{}': {}",
                    key_spec.name, e
                ))
            })?;

        // Envoy expects 80-byte keys
        if decoded.len() != 80 {
            return Err(Error::config(format!(
                "Session ticket key '{}' must be 80 bytes, got {} bytes",
                key_spec.name,
                decoded.len()
            )));
        }

        ticket_keys.push(DataSource {
            specifier: Some(Specifier::InlineBytes(decoded)),
            ..Default::default()
        });
    }

    Ok(secret::Type::SessionTicketKeys(TlsSessionTicketKeys { keys: ticket_keys }))
}

/// Convert domain StringMatcher to Envoy StringMatcher
fn convert_string_matcher(matcher: StringMatcher) -> Result<EnvoyStringMatcher> {
    let match_pattern = match matcher {
        StringMatcher::Exact { value } => MatchPattern::Exact(value),
        StringMatcher::Prefix { value } => MatchPattern::Prefix(value),
        StringMatcher::Suffix { value } => MatchPattern::Suffix(value),
        StringMatcher::SafeRegex { pattern } => {
            MatchPattern::SafeRegex(envoy_types::pb::envoy::r#type::matcher::v3::RegexMatcher {
                regex: pattern,
                engine_type: None,
            })
        }
        StringMatcher::Contains { value } => MatchPattern::Contains(value),
    };

    Ok(EnvoyStringMatcher { match_pattern: Some(match_pattern), ignore_case: false })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{GenericSecretSpec, SecretType, TlsCertificateSpec};

    fn make_test_secret_data(
        name: &str,
        secret_type: SecretType,
        config: SecretSpec,
    ) -> SecretData {
        SecretData {
            id: crate::domain::SecretId::new(),
            name: name.to_string(),
            secret_type,
            description: None,
            configuration: serde_json::to_string(&config).unwrap(),
            version: 1,
            source: "native_api".to_string(),
            team: "test-team".to_string(),
            team_name: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            expires_at: None,
            backend: None,
            reference: None,
            reference_version: None,
        }
    }

    #[test]
    fn test_build_generic_secret() {
        let secret_value = base64::engine::general_purpose::STANDARD.encode(b"my-oauth-secret");
        let spec = SecretSpec::GenericSecret(GenericSecretSpec { secret: secret_value });
        let data = make_test_secret_data("oauth-token", SecretType::GenericSecret, spec);

        let secret = build_secret(&data).unwrap();
        assert_eq!(secret.name, "oauth-token");
        assert!(matches!(secret.r#type, Some(secret::Type::GenericSecret(_))));
    }

    #[test]
    fn test_build_tls_certificate() {
        let spec = SecretSpec::TlsCertificate(TlsCertificateSpec {
            certificate_chain: "-----BEGIN CERTIFICATE-----\nMIIC...\n-----END CERTIFICATE-----"
                .to_string(),
            private_key: "-----BEGIN PRIVATE KEY-----\nMIIE...\n-----END PRIVATE KEY-----"
                .to_string(),
            password: None,
            ocsp_staple: None,
        });
        let data = make_test_secret_data("server-cert", SecretType::TlsCertificate, spec);

        let secret = build_secret(&data).unwrap();
        assert_eq!(secret.name, "server-cert");
        assert!(matches!(secret.r#type, Some(secret::Type::TlsCertificate(_))));
    }

    #[test]
    fn test_build_validation_context() {
        let spec = SecretSpec::CertificateValidationContext(CertificateValidationContextSpec {
            trusted_ca: "-----BEGIN CERTIFICATE-----\nMIIC...\n-----END CERTIFICATE-----"
                .to_string(),
            match_subject_alt_names: vec![],
            crl: None,
            only_verify_leaf_cert_crl: false,
        });
        let data =
            make_test_secret_data("client-ca", SecretType::CertificateValidationContext, spec);

        let secret = build_secret(&data).unwrap();
        assert_eq!(secret.name, "client-ca");
        assert!(matches!(secret.r#type, Some(secret::Type::ValidationContext(_))));
    }

    #[test]
    fn test_build_session_ticket_keys() {
        let key = base64::engine::general_purpose::STANDARD.encode(vec![0u8; 80]);
        let spec = SecretSpec::SessionTicketKeys {
            keys: vec![crate::domain::SessionTicketKeySpec { name: "key1".to_string(), key }],
        };
        let data = make_test_secret_data("ticket-keys", SecretType::SessionTicketKeys, spec);

        let secret = build_secret(&data).unwrap();
        assert_eq!(secret.name, "ticket-keys");
        assert!(matches!(secret.r#type, Some(secret::Type::SessionTicketKeys(_))));
    }

    #[test]
    fn test_secrets_from_database_entries() {
        let secret_value = base64::engine::general_purpose::STANDARD.encode(b"secret1");
        let entries = vec![make_test_secret_data(
            "secret1",
            SecretType::GenericSecret,
            SecretSpec::GenericSecret(GenericSecretSpec { secret: secret_value }),
        )];

        let resources = secrets_from_database_entries(entries, "test").unwrap();
        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0].name, "secret1");
        assert_eq!(resources[0].type_url(), SECRET_TYPE_URL);
    }

    #[test]
    fn test_type_mismatch_error() {
        // Create a secret with mismatched type
        let data = SecretData {
            id: crate::domain::SecretId::new(),
            name: "bad-secret".to_string(),
            secret_type: SecretType::TlsCertificate, // Says TLS cert
            description: None,
            configuration: serde_json::to_string(&SecretSpec::GenericSecret(GenericSecretSpec {
                secret: "dGVzdA==".to_string(),
            }))
            .unwrap(), // But config is generic secret
            version: 1,
            source: "native_api".to_string(),
            team: "test-team".to_string(),
            team_name: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            expires_at: None,
            backend: None,
            reference: None,
            reference_version: None,
        };

        let result = build_secret(&data);
        assert!(result.is_err());
    }
}
