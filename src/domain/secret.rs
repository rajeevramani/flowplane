//! Secret domain types for SDS (Secret Discovery Service)
//!
//! This module contains domain entities for managing Envoy secrets including
//! TLS certificates, generic secrets (OAuth tokens, API keys), and validation contexts.
//!
//! ## Secret Types
//!
//! - **GenericSecret**: For OAuth2 tokens, API keys, HMAC secrets
//! - **TlsCertificate**: Public/private key pairs for TLS
//! - **CertificateValidationContext**: CA certificates for peer verification
//! - **SessionTicketKeys**: For TLS session resumption

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use utoipa::ToSchema;

/// Secret type enumeration matching Envoy's SDS specification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SecretType {
    /// Generic secret for OAuth2 tokens, API keys, HMAC keys
    GenericSecret,
    /// TLS certificate with private key
    TlsCertificate,
    /// Certificate validation context (CA certs for verification)
    CertificateValidationContext,
    /// Session ticket keys for TLS resumption
    SessionTicketKeys,
}

impl SecretType {
    /// Get the database representation of this type
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::GenericSecret => "generic_secret",
            Self::TlsCertificate => "tls_certificate",
            Self::CertificateValidationContext => "certificate_validation_context",
            Self::SessionTicketKeys => "session_ticket_keys",
        }
    }
}

impl FromStr for SecretType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "generic_secret" => Ok(Self::GenericSecret),
            "tls_certificate" => Ok(Self::TlsCertificate),
            "certificate_validation_context" => Ok(Self::CertificateValidationContext),
            "session_ticket_keys" => Ok(Self::SessionTicketKeys),
            _ => Err(format!("Unknown secret type: {}", s)),
        }
    }
}

impl fmt::Display for SecretType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Generic secret specification (OAuth2 tokens, API keys, HMAC secrets)
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct GenericSecretSpec {
    /// Base64-encoded secret value
    pub secret: String,
}

/// TLS certificate specification
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TlsCertificateSpec {
    /// PEM-encoded certificate chain
    pub certificate_chain: String,
    /// PEM-encoded private key
    pub private_key: String,
    /// Optional password for encrypted private key
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// Optional OCSP staple response (base64-encoded)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ocsp_staple: Option<String>,
}

/// String matcher for Subject Alternative Name verification
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "match_type", rename_all = "snake_case")]
pub enum StringMatcher {
    /// Exact string match
    Exact { value: String },
    /// Prefix match
    Prefix { value: String },
    /// Suffix match
    Suffix { value: String },
    /// Safe regex match
    SafeRegex { pattern: String },
    /// Contains substring
    Contains { value: String },
}

/// Certificate validation context specification
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CertificateValidationContextSpec {
    /// PEM-encoded trusted CA certificates
    pub trusted_ca: String,
    /// Subject alternative names to match
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub match_subject_alt_names: Vec<StringMatcher>,
    /// Certificate revocation list (CRL) in PEM format
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crl: Option<String>,
    /// Whether to only verify leaf certificate CRL
    #[serde(default)]
    pub only_verify_leaf_cert_crl: bool,
}

/// Session ticket key specification
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SessionTicketKeySpec {
    /// Name identifier for the key
    pub name: String,
    /// Base64-encoded 80-byte key
    pub key: String,
}

/// Unified secret configuration - type-specific data
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SecretSpec {
    /// Generic secret (OAuth2 tokens, API keys)
    GenericSecret(GenericSecretSpec),
    /// TLS certificate with private key
    TlsCertificate(TlsCertificateSpec),
    /// Certificate validation context (CA certs)
    CertificateValidationContext(CertificateValidationContextSpec),
    /// Session ticket keys for TLS resumption
    SessionTicketKeys { keys: Vec<SessionTicketKeySpec> },
}

impl SecretSpec {
    /// Get the secret type for this specification
    pub fn secret_type(&self) -> SecretType {
        match self {
            Self::GenericSecret(_) => SecretType::GenericSecret,
            Self::TlsCertificate(_) => SecretType::TlsCertificate,
            Self::CertificateValidationContext(_) => SecretType::CertificateValidationContext,
            Self::SessionTicketKeys { .. } => SecretType::SessionTicketKeys,
        }
    }

    /// Validate the secret specification
    pub fn validate(&self) -> Result<(), SecretValidationError> {
        match self {
            Self::GenericSecret(spec) => {
                if spec.secret.is_empty() {
                    return Err(SecretValidationError::EmptySecretValue);
                }
                // Validate base64 encoding
                use base64::Engine;
                base64::engine::general_purpose::STANDARD
                    .decode(&spec.secret)
                    .map_err(|_| SecretValidationError::InvalidBase64)?;
            }
            Self::TlsCertificate(spec) => {
                if spec.certificate_chain.is_empty() {
                    return Err(SecretValidationError::EmptyCertificateChain);
                }
                if spec.private_key.is_empty() {
                    return Err(SecretValidationError::EmptyPrivateKey);
                }
                // Basic PEM format validation
                if !spec.certificate_chain.contains("-----BEGIN") {
                    return Err(SecretValidationError::InvalidCertificateFormat);
                }
                if !spec.private_key.contains("-----BEGIN") {
                    return Err(SecretValidationError::InvalidPrivateKeyFormat);
                }
            }
            Self::CertificateValidationContext(spec) => {
                if spec.trusted_ca.is_empty() {
                    return Err(SecretValidationError::EmptyTrustedCa);
                }
                if !spec.trusted_ca.contains("-----BEGIN") {
                    return Err(SecretValidationError::InvalidCertificateFormat);
                }
            }
            Self::SessionTicketKeys { keys } => {
                if keys.is_empty() {
                    return Err(SecretValidationError::EmptySessionTicketKeys);
                }
                for key in keys {
                    use base64::Engine;
                    let decoded = base64::engine::general_purpose::STANDARD
                        .decode(&key.key)
                        .map_err(|_| SecretValidationError::InvalidBase64)?;
                    // Session ticket keys must be exactly 80 bytes
                    if decoded.len() != 80 {
                        return Err(SecretValidationError::InvalidSessionTicketKeyLength(
                            decoded.len(),
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

/// Secret validation errors
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretValidationError {
    /// Secret value is empty
    EmptySecretValue,
    /// Invalid base64 encoding
    InvalidBase64,
    /// Certificate chain is empty
    EmptyCertificateChain,
    /// Private key is empty
    EmptyPrivateKey,
    /// Invalid certificate PEM format
    InvalidCertificateFormat,
    /// Invalid private key PEM format
    InvalidPrivateKeyFormat,
    /// Trusted CA is empty
    EmptyTrustedCa,
    /// Session ticket keys list is empty
    EmptySessionTicketKeys,
    /// Session ticket key has invalid length (must be 80 bytes)
    InvalidSessionTicketKeyLength(usize),
}

impl fmt::Display for SecretValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptySecretValue => write!(f, "secret value cannot be empty"),
            Self::InvalidBase64 => write!(f, "invalid base64 encoding"),
            Self::EmptyCertificateChain => write!(f, "certificate chain cannot be empty"),
            Self::EmptyPrivateKey => write!(f, "private key cannot be empty"),
            Self::InvalidCertificateFormat => write!(f, "invalid certificate PEM format"),
            Self::InvalidPrivateKeyFormat => write!(f, "invalid private key PEM format"),
            Self::EmptyTrustedCa => write!(f, "trusted CA cannot be empty"),
            Self::EmptySessionTicketKeys => write!(f, "session ticket keys list cannot be empty"),
            Self::InvalidSessionTicketKeyLength(len) => {
                write!(f, "session ticket key must be 80 bytes, got {} bytes", len)
            }
        }
    }
}

impl std::error::Error for SecretValidationError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_secret_type_roundtrip() {
        for secret_type in [
            SecretType::GenericSecret,
            SecretType::TlsCertificate,
            SecretType::CertificateValidationContext,
            SecretType::SessionTicketKeys,
        ] {
            let s = secret_type.as_str();
            let parsed: SecretType = s.parse().unwrap();
            assert_eq!(secret_type, parsed);
        }
    }

    #[test]
    fn test_generic_secret_validation() {
        use base64::Engine;

        // Valid secret
        let secret = base64::engine::general_purpose::STANDARD.encode(b"my-secret-value");
        let spec = SecretSpec::GenericSecret(GenericSecretSpec { secret });
        assert!(spec.validate().is_ok());

        // Empty secret
        let spec = SecretSpec::GenericSecret(GenericSecretSpec { secret: String::new() });
        assert_eq!(spec.validate(), Err(SecretValidationError::EmptySecretValue));

        // Invalid base64
        let spec = SecretSpec::GenericSecret(GenericSecretSpec {
            secret: "not-valid-base64!!!".to_string(),
        });
        assert_eq!(spec.validate(), Err(SecretValidationError::InvalidBase64));
    }

    #[test]
    fn test_tls_certificate_validation() {
        // Valid certificate
        let spec = SecretSpec::TlsCertificate(TlsCertificateSpec {
            certificate_chain: "-----BEGIN CERTIFICATE-----\nMIIC...\n-----END CERTIFICATE-----"
                .to_string(),
            private_key: "-----BEGIN PRIVATE KEY-----\nMIIE...\n-----END PRIVATE KEY-----"
                .to_string(),
            password: None,
            ocsp_staple: None,
        });
        assert!(spec.validate().is_ok());

        // Empty certificate chain
        let spec = SecretSpec::TlsCertificate(TlsCertificateSpec {
            certificate_chain: String::new(),
            private_key: "-----BEGIN PRIVATE KEY-----\nMIIE...\n-----END PRIVATE KEY-----"
                .to_string(),
            password: None,
            ocsp_staple: None,
        });
        assert_eq!(spec.validate(), Err(SecretValidationError::EmptyCertificateChain));

        // Invalid format
        let spec = SecretSpec::TlsCertificate(TlsCertificateSpec {
            certificate_chain: "not-pem-format".to_string(),
            private_key: "-----BEGIN PRIVATE KEY-----\nMIIE...\n-----END PRIVATE KEY-----"
                .to_string(),
            password: None,
            ocsp_staple: None,
        });
        assert_eq!(spec.validate(), Err(SecretValidationError::InvalidCertificateFormat));
    }

    #[test]
    fn test_session_ticket_key_validation() {
        use base64::Engine;

        // Valid 80-byte key
        let key_bytes = vec![0u8; 80];
        let key = base64::engine::general_purpose::STANDARD.encode(&key_bytes);
        let spec = SecretSpec::SessionTicketKeys {
            keys: vec![SessionTicketKeySpec { name: "key1".to_string(), key }],
        };
        assert!(spec.validate().is_ok());

        // Empty keys list
        let spec = SecretSpec::SessionTicketKeys { keys: vec![] };
        assert_eq!(spec.validate(), Err(SecretValidationError::EmptySessionTicketKeys));

        // Wrong size key (64 bytes instead of 80)
        let key_bytes = vec![0u8; 64];
        let key = base64::engine::general_purpose::STANDARD.encode(&key_bytes);
        let spec = SecretSpec::SessionTicketKeys {
            keys: vec![SessionTicketKeySpec { name: "key1".to_string(), key }],
        };
        assert_eq!(spec.validate(), Err(SecretValidationError::InvalidSessionTicketKeyLength(64)));
    }

    #[test]
    fn test_secret_spec_type() {
        let spec = SecretSpec::GenericSecret(GenericSecretSpec { secret: "dGVzdA==".to_string() });
        assert_eq!(spec.secret_type(), SecretType::GenericSecret);

        let spec = SecretSpec::TlsCertificate(TlsCertificateSpec {
            certificate_chain: "cert".to_string(),
            private_key: "key".to_string(),
            password: None,
            ocsp_staple: None,
        });
        assert_eq!(spec.secret_type(), SecretType::TlsCertificate);
    }

    #[test]
    fn test_secret_spec_serialization() {
        let spec = SecretSpec::GenericSecret(GenericSecretSpec { secret: "dGVzdA==".to_string() });

        let json = serde_json::to_string(&spec).unwrap();
        assert!(json.contains("\"type\":\"generic_secret\""));

        let deserialized: SecretSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.secret_type(), SecretType::GenericSecret);
    }
}
