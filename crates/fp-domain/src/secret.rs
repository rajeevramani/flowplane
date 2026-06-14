//! SDS secrets (spec/04 §3.7). Secret values are accepted at the API boundary but are never
//! returned by read paths; storage encrypts the JSON representation at rest.

use crate::error::{DomainError, DomainResult};
use crate::id::{SecretId, TeamId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Secret {
    pub id: SecretId,
    pub team_id: TeamId,
    pub name: String,
    pub description: String,
    pub secret_type: SecretType,
    pub version: i64,
    pub encryption_key_id: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretType {
    GenericSecret,
    TlsCertificate,
    CertificateValidationContext,
    SessionTicketKeys,
}

impl SecretType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::GenericSecret => "generic_secret",
            Self::TlsCertificate => "tls_certificate",
            Self::CertificateValidationContext => "certificate_validation_context",
            Self::SessionTicketKeys => "session_ticket_keys",
        }
    }
}

impl std::str::FromStr for SecretType {
    type Err = DomainError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw {
            "generic_secret" => Ok(Self::GenericSecret),
            "tls_certificate" => Ok(Self::TlsCertificate),
            "certificate_validation_context" => Ok(Self::CertificateValidationContext),
            "session_ticket_keys" => Ok(Self::SessionTicketKeys),
            _ => Err(DomainError::validation(format!(
                "\"{raw}\" is not a known secret type"
            ))),
        }
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SecretSpec {
    GenericSecret {
        secret: String,
    },
    TlsCertificate {
        certificate_chain: String,
        private_key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        password: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ocsp_staple: Option<String>,
    },
    CertificateValidationContext {
        trusted_ca: String,
        #[serde(default)]
        match_subject_alt_names: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        crl: Option<String>,
        #[serde(default)]
        only_verify_leaf_cert_crl: bool,
    },
    SessionTicketKeys {
        keys: Vec<SessionTicketKey>,
    },
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionTicketKey {
    pub name: String,
    pub key: String,
}

impl fmt::Debug for SecretSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GenericSecret { secret } => f
                .debug_struct("GenericSecret")
                .field("secret", &RedactedLen(secret))
                .finish(),
            Self::TlsCertificate {
                certificate_chain,
                private_key,
                password,
                ocsp_staple,
            } => f
                .debug_struct("TlsCertificate")
                .field("certificate_chain", &RedactedLen(certificate_chain))
                .field("private_key", &RedactedLen(private_key))
                .field(
                    "password",
                    &password.as_ref().map(|value| RedactedLen(value)),
                )
                .field(
                    "ocsp_staple",
                    &ocsp_staple.as_ref().map(|value| RedactedLen(value)),
                )
                .finish(),
            Self::CertificateValidationContext {
                trusted_ca,
                match_subject_alt_names,
                crl,
                only_verify_leaf_cert_crl,
            } => f
                .debug_struct("CertificateValidationContext")
                .field("trusted_ca", &RedactedLen(trusted_ca))
                .field(
                    "match_subject_alt_names",
                    &RedactedCount(match_subject_alt_names),
                )
                .field("crl", &crl.as_ref().map(|value| RedactedLen(value)))
                .field("only_verify_leaf_cert_crl", only_verify_leaf_cert_crl)
                .finish(),
            Self::SessionTicketKeys { keys } => f
                .debug_struct("SessionTicketKeys")
                .field("keys", &RedactedCount(keys))
                .finish(),
        }
    }
}

impl fmt::Debug for SessionTicketKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionTicketKey")
            .field("name", &self.name)
            .field("key", &RedactedLen(&self.key))
            .finish()
    }
}

struct RedactedLen<'a>(&'a str);

impl fmt::Debug for RedactedLen<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "\"<redacted {} bytes>\"", self.0.len())
    }
}

struct RedactedCount<'a, T>(&'a [T]);

impl<T> fmt::Debug for RedactedCount<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "\"<redacted {} items>\"", self.0.len())
    }
}

impl SecretSpec {
    pub fn secret_type(&self) -> SecretType {
        match self {
            Self::GenericSecret { .. } => SecretType::GenericSecret,
            Self::TlsCertificate { .. } => SecretType::TlsCertificate,
            Self::CertificateValidationContext { .. } => SecretType::CertificateValidationContext,
            Self::SessionTicketKeys { .. } => SecretType::SessionTicketKeys,
        }
    }

    pub fn validate(&self) -> DomainResult<()> {
        match self {
            Self::GenericSecret { secret } => {
                if secret.is_empty() {
                    return Err(DomainError::validation("generic secret must not be empty"));
                }
                base64::Engine::decode(&base64::engine::general_purpose::STANDARD, secret)
                    .map_err(|_| DomainError::validation("generic secret must be base64"))?;
            }
            Self::TlsCertificate {
                certificate_chain,
                private_key,
                ocsp_staple,
                ..
            } => {
                if certificate_chain.trim().is_empty() || private_key.trim().is_empty() {
                    return Err(DomainError::validation(
                        "TLS certificate secrets require certificate_chain and private_key",
                    ));
                }
                if let Some(staple) = ocsp_staple {
                    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, staple)
                        .map_err(|_| DomainError::validation("ocsp_staple must be base64"))?;
                }
            }
            Self::CertificateValidationContext { trusted_ca, .. } => {
                if trusted_ca.trim().is_empty() {
                    return Err(DomainError::validation(
                        "certificate validation context requires trusted_ca",
                    ));
                }
            }
            Self::SessionTicketKeys { keys } => {
                if keys.is_empty() {
                    return Err(DomainError::validation(
                        "session ticket keys must include at least one key",
                    ));
                }
                for key in keys {
                    let decoded = base64::Engine::decode(
                        &base64::engine::general_purpose::STANDARD,
                        &key.key,
                    )
                    .map_err(|_| DomainError::validation("session ticket key must be base64"))?;
                    if decoded.len() != 80 {
                        return Err(DomainError::validation(
                            "session ticket key must decode to exactly 80 bytes",
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{SecretSpec, SessionTicketKey};

    #[test]
    fn secret_spec_debug_redacts_generic_secret_value() {
        let spec = SecretSpec::GenericSecret {
            secret: "plain-secret".to_string(),
        };

        let debug = format!("{spec:?}");

        assert!(debug.contains("GenericSecret"));
        assert!(debug.contains("<redacted 12 bytes>"));
        assert!(!debug.contains("plain-secret"));
    }

    #[test]
    fn secret_spec_debug_redacts_tls_secret_values() {
        let spec = SecretSpec::TlsCertificate {
            certificate_chain: "certificate-chain".to_string(),
            private_key: "private-key".to_string(),
            password: Some("key-password".to_string()),
            ocsp_staple: Some("ocsp-staple".to_string()),
        };

        let debug = format!("{spec:?}");

        assert!(debug.contains("TlsCertificate"));
        for sensitive in [
            "certificate-chain",
            "private-key",
            "key-password",
            "ocsp-staple",
        ] {
            assert!(!debug.contains(sensitive), "{sensitive} leaked in {debug}");
        }
    }

    #[test]
    fn secret_spec_debug_redacts_validation_context_values() {
        let spec = SecretSpec::CertificateValidationContext {
            trusted_ca: "trusted-ca".to_string(),
            match_subject_alt_names: vec!["api.internal".to_string()],
            crl: Some("revocation-list".to_string()),
            only_verify_leaf_cert_crl: true,
        };

        let debug = format!("{spec:?}");

        assert!(debug.contains("CertificateValidationContext"));
        assert!(debug.contains("only_verify_leaf_cert_crl: true"));
        assert!(!debug.contains("trusted-ca"));
        assert!(!debug.contains("api.internal"));
        assert!(!debug.contains("revocation-list"));
    }

    #[test]
    fn session_ticket_key_debug_redacts_key_value() {
        let key = SessionTicketKey {
            name: "ticket-key-1".to_string(),
            key: "ticket-secret-material".to_string(),
        };

        let debug = format!("{key:?}");

        assert!(debug.contains("ticket-key-1"));
        assert!(debug.contains("<redacted 22 bytes>"));
        assert!(!debug.contains("ticket-secret-material"));
    }

    #[test]
    fn secret_spec_debug_redacts_session_ticket_key_values() {
        let spec = SecretSpec::SessionTicketKeys {
            keys: vec![SessionTicketKey {
                name: "ticket-key-1".to_string(),
                key: "ticket-secret-material".to_string(),
            }],
        };

        let debug = format!("{spec:?}");

        assert!(debug.contains("SessionTicketKeys"));
        assert!(debug.contains("<redacted 1 items>"));
        assert!(!debug.contains("ticket-key-1"));
        assert!(!debug.contains("ticket-secret-material"));
    }
}
