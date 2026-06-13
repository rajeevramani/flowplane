//! SDS secrets (spec/04 §3.7). Secret values are accepted at the API boundary but are never
//! returned by read paths; storage encrypts the JSON representation at rest.

use crate::error::{DomainError, DomainResult};
use crate::id::{SecretId, TeamId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionTicketKey {
    pub name: String,
    pub key: String,
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
