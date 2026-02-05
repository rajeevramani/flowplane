//! Certificate backend abstraction for mTLS certificate generation.
//!
//! This module provides a pluggable architecture for generating proxy certificates
//! from various PKI backends (Vault, AWS ACM Private CA, GCP CAS, etc.).
//!
//! # Architecture
//!
//! The module follows the same pattern as the secret backends:
//! - `CertificateBackend` trait defines the interface
//! - Concrete implementations (VaultPkiBackend, MockCertificateBackend) implement the trait
//! - The `SecretBackendRegistry` holds the active certificate backend
//!
//! # SPIFFE Identity
//!
//! All generated certificates include a SPIFFE URI in the Subject Alternative Name (SAN):
//! ```text
//! spiffe://{trust_domain}/team/{team}/proxy/{proxy_id}
//! ```
//!
//! The `team` component is critical for multi-tenant authorization - it determines
//! which resources the proxy can access via xDS.
//!
//! # Available Backends
//!
//! - **VaultPkiBackend**: Production backend using HashiCorp Vault PKI
//! - **MockCertificateBackend**: Testing backend with deterministic outputs
//! - **AwsAcmPca**: (Future) AWS ACM Private CA
//! - **GcpCas**: (Future) GCP Certificate Authority Service
//!
//! # Example
//!
//! ```rust,ignore
//! use flowplane::secrets::backends::certificates::{CertificateBackend, VaultPkiBackend};
//!
//! // Get backend from registry
//! let backend = registry.certificate_backend()
//!     .ok_or_else(|| "Certificate backend not configured")?;
//!
//! // Generate certificate
//! let cert = backend.generate_certificate("engineering", "proxy-1", Some(720)).await?;
//!
//! // Use certificate for mTLS
//! println!("Certificate SPIFFE URI: {}", cert.spiffe_uri);
//! ```
//!
//! # Testing
//!
//! Use `MockCertificateBackend` for unit and integration tests:
//!
//! ```rust,ignore
//! use flowplane::secrets::backends::certificates::MockCertificateBackend;
//!
//! let mock = MockCertificateBackend::new("test.local");
//!
//! // Generate deterministic test certificate
//! let cert = mock.generate_certificate("team", "proxy", None).await?;
//! assert!(cert.certificate.contains("MOCK-CERT"));
//!
//! // Test error handling
//! mock.set_fail_next(true);
//! let result = mock.generate_certificate("team", "proxy", None).await;
//! assert!(result.is_err());
//! ```

mod backend;
mod mock;
mod vault_pki;

pub use backend::{CertificateBackend, CertificateBackendType, RetryConfig};
pub use mock::MockCertificateBackend;
pub use vault_pki::VaultPkiBackend;
