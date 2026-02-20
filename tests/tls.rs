//! TLS and Certificate tests for Flowplane.
//!
//! This test module includes:
//! - Unit tests for certificate parsing and validation
//! - Unit tests for the TestCertificateAuthority (mTLS e2e support)
//! - Integration tests for API TLS configuration

mod tls {
    pub mod integration;
    pub mod support;
    pub mod unit;
}

// Re-export for convenience
pub use tls::support::*;
