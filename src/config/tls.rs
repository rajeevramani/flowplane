use std::path::PathBuf;

use crate::{errors::TlsError, Result};

/// TLS configuration for the admin API listener.
#[derive(Debug, Clone)]
pub struct ApiTlsConfig {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
    pub chain_path: Option<PathBuf>,
}

impl ApiTlsConfig {
    /// Load TLS configuration for the admin API from environment variables.
    pub fn from_env() -> Result<Option<Self>> {
        let enabled = std::env::var("FLOWPLANE_API_TLS_ENABLED")
            .ok()
            .map(|value| matches!(value.to_lowercase().as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(false);

        if !enabled {
            return Ok(None);
        }

        let cert_path = std::env::var("FLOWPLANE_API_TLS_CERT_PATH")
            .map_err(|_| TlsError::MissingCertificatePath)?
            .trim()
            .to_string();

        if cert_path.is_empty() {
            return Err(TlsError::MissingCertificatePath.into());
        }

        let key_path = std::env::var("FLOWPLANE_API_TLS_KEY_PATH")
            .map_err(|_| TlsError::MissingPrivateKeyPath)?
            .trim()
            .to_string();

        if key_path.is_empty() {
            return Err(TlsError::MissingPrivateKeyPath.into());
        }

        let chain_path = std::env::var("FLOWPLANE_API_TLS_CHAIN_PATH")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);

        Ok(Some(Self {
            cert_path: PathBuf::from(cert_path),
            key_path: PathBuf::from(key_path),
            chain_path,
        }))
    }
}
