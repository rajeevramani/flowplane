use std::path::PathBuf;

use chrono::{DateTime, Utc};
use thiserror::Error;

/// TLS-specific error variants surfaced during configuration and certificate loading.
#[derive(Debug, Error)]
pub enum TlsError {
    /// TLS has been enabled but the certificate path was not provided.
    #[error("TLS is enabled but certificate path is not configured")]
    MissingCertificatePath,

    /// TLS has been enabled but the private key path was not provided.
    #[error("TLS is enabled but private key path is not configured")]
    MissingPrivateKeyPath,

    /// The optional chain path was supplied but the file could not be read.
    #[error("Failed to read certificate chain at {path}: {source}")]
    ChainReadError {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// The certificate file could not be read.
    #[error("Failed to read certificate at {path}: {source}")]
    CertificateReadError {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// The private key file could not be read.
    #[error("Failed to read private key at {path}: {source}")]
    PrivateKeyReadError {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// No certificates were found in the supplied PEM file.
    #[error("Certificate file {path} does not contain any certificates")]
    EmptyCertificateChain { path: PathBuf },

    /// The certificate PEM contents were invalid or unreadable.
    #[error("Certificate file {path} is not a valid PEM: {source}")]
    InvalidCertificatePem {
        path: PathBuf,
        #[source]
        source: anyhow::Error,
    },

    /// The certificate chain contained a malformed certificate.
    #[error("Certificate chain file {path} is not a valid PEM: {source}")]
    InvalidChainPem {
        path: PathBuf,
        #[source]
        source: anyhow::Error,
    },

    /// The private key PEM contents were invalid or unsupported.
    #[error("Private key file {path} does not contain a supported private key")]
    InvalidPrivateKey {
        path: PathBuf,
        #[source]
        source: Option<anyhow::Error>,
    },

    /// The supplied certificate and key do not match.
    #[error("Certificate and private key do not match")]
    CertificateKeyMismatch,

    /// The certificate is not yet valid.
    #[error("Certificate at {path} is not valid before {not_before}")]
    CertificateNotYetValid { path: PathBuf, not_before: DateTime<Utc> },

    /// The certificate is expired.
    #[error("Certificate at {path} expired at {not_after}")]
    CertificateExpired { path: PathBuf, not_after: DateTime<Utc> },

    /// Generic metadata extraction failure.
    #[error("Failed to extract certificate metadata from {path}: {source}")]
    CertificateMetadata {
        path: PathBuf,
        #[source]
        source: anyhow::Error,
    },
}
