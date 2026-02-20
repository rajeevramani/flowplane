//! mTLS Identity Extraction for xDS Client Connections
//!
//! This module provides functionality to extract SPIFFE identities from
//! client certificates presented during mTLS connections. When mTLS is enabled,
//! the team identity is extracted from the certificate's SPIFFE URI rather than
//! trusting the node metadata.
//!
//! # SPIFFE URI Format
//!
//! Certificates issued by Flowplane contain a SPIFFE URI in the Subject Alternative Name.
//! Both formats are supported:
//! ```text
//! spiffe://{trust_domain}/org/{org}/team/{team}/proxy/{proxy_id}   (org-scoped)
//! spiffe://{trust_domain}/team/{team}/proxy/{proxy_id}             (legacy)
//! ```
//!
//! # Security Model
//!
//! When mTLS is enabled:
//! - Team identity comes from the validated client certificate (trusted)
//! - Node metadata team is logged but not used for authorization
//! - Connections without valid certificates are rejected
//!
//! When mTLS is disabled:
//! - Team identity comes from node.metadata.team (current behavior)
//! - This mode is suitable for development but not recommended for production

use tonic::transport::CertificateDer;
use x509_parser::prelude::*;

/// Identity information extracted from a client certificate.
#[derive(Debug, Clone)]
pub struct ClientIdentity {
    /// Organization name extracted from SPIFFE URI (None for legacy format)
    pub org: Option<String>,

    /// Team name extracted from SPIFFE URI
    pub team: String,

    /// Proxy ID extracted from SPIFFE URI
    pub proxy_id: String,

    /// Full SPIFFE URI from the certificate
    pub spiffe_uri: String,

    /// Certificate serial number for audit purposes
    pub serial_number: String,
}

/// Extract client identity from peer certificates.
///
/// # Arguments
///
/// * `peer_certs` - Slice of DER-encoded certificates from tonic's peer_certs()
///
/// # Returns
///
/// * `Some(ClientIdentity)` - If a valid SPIFFE URI was found in the first certificate
/// * `None` - If no certificates were provided or no SPIFFE URI was found
///
/// # Example
///
/// ```ignore
/// let identity = extract_client_identity(request.peer_certs().unwrap_or(&[]));
/// if let Some(id) = identity {
///     tracing::info!(team = %id.team, proxy_id = %id.proxy_id, "Client authenticated via mTLS");
/// }
/// ```
pub fn extract_client_identity(peer_certs: &[CertificateDer<'_>]) -> Option<ClientIdentity> {
    // Get the first certificate (client's certificate in the chain)
    let cert = peer_certs.first()?;

    // Parse the X.509 certificate
    let (_, parsed) = X509Certificate::from_der(cert.as_ref()).ok()?;

    // Get serial number for audit purposes
    let serial_number = format!("{:x}", parsed.serial);

    // Look for SPIFFE URI in Subject Alternative Names
    let spiffe_uri = extract_spiffe_uri_from_cert(&parsed)?;

    // Parse team and proxy_id from SPIFFE URI (supports both legacy and org-scoped formats)
    let (team, proxy_id) = parse_spiffe_uri(&spiffe_uri)?;
    let org = crate::secrets::parse_org_from_spiffe_uri(&spiffe_uri);

    Some(ClientIdentity { org, team, proxy_id, spiffe_uri, serial_number })
}

/// Extract SPIFFE URI from certificate's Subject Alternative Name extension.
fn extract_spiffe_uri_from_cert(cert: &X509Certificate<'_>) -> Option<String> {
    // Find the Subject Alternative Name extension
    for ext in cert.extensions() {
        if let ParsedExtension::SubjectAlternativeName(san) = ext.parsed_extension() {
            for name in &san.general_names {
                if let GeneralName::URI(uri) = name {
                    if uri.starts_with("spiffe://") {
                        return Some(uri.to_string());
                    }
                }
            }
        }
    }
    None
}

/// Parse team and proxy_id from a SPIFFE URI.
///
/// Supports both formats:
/// - New: `spiffe://{trust_domain}/org/{org}/team/{team}/proxy/{proxy_id}`
/// - Legacy: `spiffe://{trust_domain}/team/{team}/proxy/{proxy_id}`
fn parse_spiffe_uri(uri: &str) -> Option<(String, String)> {
    // Use the parsing functions from our vault module
    let team = crate::secrets::parse_team_from_spiffe_uri(uri)?;
    let proxy_id = crate::secrets::parse_proxy_id_from_spiffe_uri(uri)?;
    Some((team, proxy_id))
}

/// Check if mTLS is enabled for xDS connections.
///
/// mTLS is considered enabled if:
/// 1. FLOWPLANE_XDS_TLS_CERT_PATH is set (TLS enabled)
/// 2. FLOWPLANE_XDS_TLS_CLIENT_CA_PATH is set (client verification enabled)
/// 3. FLOWPLANE_XDS_TLS_REQUIRE_CLIENT_CERT is not explicitly "false"
pub fn is_xds_mtls_enabled() -> bool {
    // Check if TLS is configured
    let tls_enabled =
        std::env::var("FLOWPLANE_XDS_TLS_CERT_PATH").ok().filter(|v| !v.is_empty()).is_some();

    if !tls_enabled {
        return false;
    }

    // Check if client CA is configured
    let client_ca_configured =
        std::env::var("FLOWPLANE_XDS_TLS_CLIENT_CA_PATH").ok().filter(|v| !v.is_empty()).is_some();

    if !client_ca_configured {
        return false;
    }

    // Check if client cert is required (default true)
    let require_client_cert = std::env::var("FLOWPLANE_XDS_TLS_REQUIRE_CLIENT_CERT")
        .ok()
        .map(|v| !matches!(v.to_lowercase().as_str(), "false" | "0" | "no" | "off"))
        .unwrap_or(true);

    require_client_cert
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_spiffe_uri_valid() {
        let uri = "spiffe://flowplane.local/team/engineering/proxy/envoy-1";
        let result = parse_spiffe_uri(uri);
        assert!(result.is_some());
        let (team, proxy_id) = result.unwrap();
        assert_eq!(team, "engineering");
        assert_eq!(proxy_id, "envoy-1");
    }

    #[test]
    fn test_parse_spiffe_uri_org_scoped() {
        let uri = "spiffe://flowplane.local/org/acme/team/engineering/proxy/envoy-1";
        let result = parse_spiffe_uri(uri);
        assert!(result.is_some());
        let (team, proxy_id) = result.unwrap();
        assert_eq!(team, "engineering");
        assert_eq!(proxy_id, "envoy-1");
    }

    #[test]
    fn test_parse_spiffe_uri_invalid() {
        // Missing proxy segment
        let uri = "spiffe://flowplane.local/team/engineering";
        assert!(parse_spiffe_uri(uri).is_none());

        // Not a SPIFFE URI
        let uri = "https://example.com";
        assert!(parse_spiffe_uri(uri).is_none());

        // Wrong format
        let uri = "spiffe://domain/foo/bar";
        assert!(parse_spiffe_uri(uri).is_none());
    }

    #[test]
    fn test_is_xds_mtls_enabled_default() {
        // Clear env vars
        std::env::remove_var("FLOWPLANE_XDS_TLS_CERT_PATH");
        std::env::remove_var("FLOWPLANE_XDS_TLS_CLIENT_CA_PATH");
        std::env::remove_var("FLOWPLANE_XDS_TLS_REQUIRE_CLIENT_CERT");

        // mTLS should be disabled when no TLS config
        assert!(!is_xds_mtls_enabled());
    }

    #[test]
    fn test_is_xds_mtls_enabled_tls_only() {
        std::env::set_var("FLOWPLANE_XDS_TLS_CERT_PATH", "/path/to/cert.pem");
        std::env::remove_var("FLOWPLANE_XDS_TLS_CLIENT_CA_PATH");
        std::env::remove_var("FLOWPLANE_XDS_TLS_REQUIRE_CLIENT_CERT");

        // mTLS should be disabled when only server TLS is configured
        assert!(!is_xds_mtls_enabled());

        // Cleanup
        std::env::remove_var("FLOWPLANE_XDS_TLS_CERT_PATH");
    }

    #[test]
    fn test_is_xds_mtls_enabled_full() {
        std::env::set_var("FLOWPLANE_XDS_TLS_CERT_PATH", "/path/to/cert.pem");
        std::env::set_var("FLOWPLANE_XDS_TLS_CLIENT_CA_PATH", "/path/to/ca.pem");
        std::env::remove_var("FLOWPLANE_XDS_TLS_REQUIRE_CLIENT_CERT");

        // mTLS should be enabled
        assert!(is_xds_mtls_enabled());

        // Cleanup
        std::env::remove_var("FLOWPLANE_XDS_TLS_CERT_PATH");
        std::env::remove_var("FLOWPLANE_XDS_TLS_CLIENT_CA_PATH");
    }

    #[test]
    fn test_is_xds_mtls_enabled_explicitly_disabled() {
        std::env::set_var("FLOWPLANE_XDS_TLS_CERT_PATH", "/path/to/cert.pem");
        std::env::set_var("FLOWPLANE_XDS_TLS_CLIENT_CA_PATH", "/path/to/ca.pem");
        std::env::set_var("FLOWPLANE_XDS_TLS_REQUIRE_CLIENT_CERT", "false");

        // mTLS should be disabled when explicitly turned off
        assert!(!is_xds_mtls_enabled());

        // Cleanup
        std::env::remove_var("FLOWPLANE_XDS_TLS_CERT_PATH");
        std::env::remove_var("FLOWPLANE_XDS_TLS_CLIENT_CA_PATH");
        std::env::remove_var("FLOWPLANE_XDS_TLS_REQUIRE_CLIENT_CERT");
    }
}
