use std::fs;
use x509_parser::prelude::*;

use super::super::support::TestCertificateAuthority;

// Use explicit time crate to avoid conflict with x509_parser::time
type Duration = ::time::Duration;

#[test]
fn test_certificate_authority_creation() -> anyhow::Result<()> {
    let ca = TestCertificateAuthority::new("Test CA", Duration::days(365))?;

    // Verify CA cert file exists
    assert!(ca.ca_cert_path.exists(), "CA cert file should exist");

    // Verify PEM format
    let pem = ca.ca_cert_pem();
    assert!(pem.contains("BEGIN CERTIFICATE"), "Should be PEM format");
    assert!(pem.contains("END CERTIFICATE"), "Should be PEM format");

    Ok(())
}

#[test]
fn test_issue_server_certificate() -> anyhow::Result<()> {
    let ca = TestCertificateAuthority::new("Test CA", Duration::days(365))?;

    let server_cert =
        ca.issue_server_cert(&["localhost", "xds.flowplane.local"], Duration::days(30))?;

    // Verify files exist
    assert!(server_cert.cert_path.exists(), "Server cert should exist");
    assert!(server_cert.key_path.exists(), "Server key should exist");

    // Verify PEM format
    let cert_pem = fs::read_to_string(&server_cert.cert_path)?;
    assert!(cert_pem.contains("BEGIN CERTIFICATE"), "Should be PEM format");

    Ok(())
}

#[test]
fn test_issue_client_certificate_with_spiffe_uri() -> anyhow::Result<()> {
    let ca = TestCertificateAuthority::new("Test CA", Duration::days(365))?;

    let spiffe_uri =
        TestCertificateAuthority::build_spiffe_uri("flowplane.local", "engineering", "envoy-1")?;

    let client_cert = ca.issue_client_cert(&spiffe_uri, "envoy-1", Duration::days(30))?;

    // Verify files exist
    assert!(client_cert.cert_path.exists(), "Client cert should exist");
    assert!(client_cert.key_path.exists(), "Client key should exist");

    // Verify PEM format
    let cert_pem = fs::read_to_string(&client_cert.cert_path)?;
    assert!(cert_pem.contains("BEGIN CERTIFICATE"), "Should be PEM format");

    Ok(())
}

#[test]
fn test_build_spiffe_uri() -> anyhow::Result<()> {
    let uri = TestCertificateAuthority::build_spiffe_uri("flowplane.local", "eng", "proxy-1")?;
    assert_eq!(uri, "spiffe://flowplane.local/team/eng/proxy/proxy-1");
    Ok(())
}

#[test]
fn test_build_spiffe_uri_rejects_slash_in_team() {
    // Security test: Team name with '/' should be rejected
    let result =
        TestCertificateAuthority::build_spiffe_uri("flowplane.local", "team/admin", "proxy-1");
    assert!(result.is_err(), "Should reject team name with '/'");
    let err = result.unwrap_err().to_string();
    assert!(err.contains("Team name cannot contain '/'"), "Error should mention team validation");
}

#[test]
fn test_build_spiffe_uri_rejects_empty_components() {
    // Security test: Empty components should be rejected
    let result = TestCertificateAuthority::build_spiffe_uri("", "team", "proxy");
    assert!(result.is_err(), "Should reject empty trust domain");

    let result = TestCertificateAuthority::build_spiffe_uri("domain", "", "proxy");
    assert!(result.is_err(), "Should reject empty team");

    let result = TestCertificateAuthority::build_spiffe_uri("domain", "team", "");
    assert!(result.is_err(), "Should reject empty proxy ID");
}

#[test]
fn test_parse_team_from_spiffe_uri() {
    // Valid URI format
    let uri = "spiffe://flowplane.local/team/engineering/proxy/envoy-1";
    let team = TestCertificateAuthority::parse_team_from_spiffe_uri(uri);
    assert_eq!(team, Some("engineering".to_string()));

    // Invalid format - missing team segment
    let uri_invalid = "spiffe://flowplane.local/invalid/format";
    let team_invalid = TestCertificateAuthority::parse_team_from_spiffe_uri(uri_invalid);
    assert!(team_invalid.is_none(), "Should return None for invalid format");
}

#[test]
fn test_spiffe_uri_in_certificate_san() -> anyhow::Result<()> {
    let ca = TestCertificateAuthority::new("Test CA", Duration::days(365))?;

    let spiffe_uri = "spiffe://flowplane.local/team/qa/proxy/test-proxy";
    let client_cert = ca.issue_client_cert(spiffe_uri, "test-proxy", Duration::days(30))?;

    // Parse the certificate to verify SPIFFE URI is in SAN
    let cert_pem = fs::read_to_string(&client_cert.cert_path)?;

    // Extract DER from PEM
    let (_, pem) = x509_parser::pem::parse_x509_pem(cert_pem.as_bytes())?;
    let (_, cert) = X509Certificate::from_der(&pem.contents)?;

    // Find SAN extension and verify SPIFFE URI
    let san =
        cert.subject_alternative_name()?.ok_or_else(|| anyhow::anyhow!("SAN should be present"))?;

    let uri_found = san.value.general_names.iter().any(|name| {
        if let x509_parser::extensions::GeneralName::URI(uri) = name {
            *uri == spiffe_uri
        } else {
            false
        }
    });

    assert!(uri_found, "SPIFFE URI should be in certificate SAN");

    Ok(())
}

#[test]
fn test_ca_can_sign_multiple_certificates() -> anyhow::Result<()> {
    let ca = TestCertificateAuthority::new("Test CA", Duration::days(365))?;

    // Issue multiple certificates
    let server1 = ca.issue_server_cert(&["server1.local"], Duration::days(30))?;
    let server2 = ca.issue_server_cert(&["server2.local"], Duration::days(30))?;

    let client1 =
        ca.issue_client_cert("spiffe://flowplane.local/team/a/proxy/p1", "p1", Duration::days(30))?;
    let client2 =
        ca.issue_client_cert("spiffe://flowplane.local/team/b/proxy/p2", "p2", Duration::days(30))?;

    // Verify all certificates exist
    assert!(server1.cert_path.exists());
    assert!(server2.cert_path.exists());
    assert!(client1.cert_path.exists());
    assert!(client2.cert_path.exists());

    Ok(())
}

#[test]
fn test_certificate_chain_validation() -> anyhow::Result<()> {
    let ca = TestCertificateAuthority::new("Test CA", Duration::days(365))?;

    let client_cert = ca.issue_client_cert(
        "spiffe://flowplane.local/team/eng/proxy/test",
        "test",
        Duration::days(30),
    )?;

    // Parse both certificates
    let ca_pem = ca.ca_cert_pem();
    let (_, ca_pem_parsed) = x509_parser::pem::parse_x509_pem(ca_pem.as_bytes())?;
    let (_, ca_cert) = X509Certificate::from_der(&ca_pem_parsed.contents)?;

    let client_pem = fs::read_to_string(&client_cert.cert_path)?;
    let (_, client_pem_parsed) = x509_parser::pem::parse_x509_pem(client_pem.as_bytes())?;
    let (_, client_x509) = X509Certificate::from_der(&client_pem_parsed.contents)?;

    // Verify client cert's issuer matches CA's subject
    assert_eq!(
        client_x509.issuer(),
        ca_cert.subject(),
        "Client cert issuer should match CA subject"
    );

    // Verify CA is marked as CA
    let basic_constraints = ca_cert
        .basic_constraints()?
        .ok_or_else(|| anyhow::anyhow!("Basic constraints should be present"))?;
    assert!(basic_constraints.value.ca, "CA should be marked as CA");

    Ok(())
}
