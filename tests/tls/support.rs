use std::{fs, net::Ipv4Addr, path::PathBuf};

use anyhow::Context;
use rcgen::{BasicConstraints, CertificateParams, DnType, Ia5String, IsCa, KeyPair, SanType};
use tempfile::TempDir;
use time::Duration;

/// Helper for generating ephemeral certificate files for integration tests.
pub struct TestCertificateFiles {
    #[allow(dead_code)]
    temp_dir: TempDir,
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
}

impl TestCertificateFiles {
    /// Generate a localhost certificate valid for the provided duration.
    pub fn localhost(valid_for: Duration) -> anyhow::Result<Self> {
        let mut params =
            CertificateParams::new(vec!["localhost".into()]).context("build certificate params")?;

        params.distinguished_name.push(DnType::CommonName, "Flowplane Test");
        params.distinguished_name.push(DnType::OrganizationName, "Flowplane");

        params.subject_alt_names.push(SanType::IpAddress(Ipv4Addr::LOCALHOST.into()));

        let now = time::OffsetDateTime::now_utc();
        // Use minimal backdating (5 minutes) for clock skew tolerance
        // Security: Reduces temporal attack surface vs 1-day backdating
        params.not_before = now - Duration::minutes(5);
        params.not_after = now + valid_for;

        Self::from_params(params)
    }

    pub fn with_expiration(not_after: time::OffsetDateTime) -> anyhow::Result<Self> {
        let mut params =
            CertificateParams::new(vec!["localhost".into()]).context("build certificate params")?;

        params.distinguished_name.push(DnType::CommonName, "Flowplane Test");
        params.distinguished_name.push(DnType::OrganizationName, "Flowplane");
        params.subject_alt_names.push(SanType::IpAddress(Ipv4Addr::LOCALHOST.into()));

        // Use minimal backdating (5 minutes) for clock skew tolerance
        params.not_before = time::OffsetDateTime::now_utc() - Duration::minutes(5);
        params.not_after = not_after;

        Self::from_params(params)
    }

    fn from_params(params: CertificateParams) -> anyhow::Result<Self> {
        // Generate key pair (uses ECDSA P-256 by default in rcgen 0.13)
        let key_pair = KeyPair::generate().context("generate key pair")?;

        // Self-sign the certificate
        let cert = params.self_signed(&key_pair).context("self-sign certificate")?;

        let temp_dir = TempDir::new().context("create temp dir")?;

        let cert_path = temp_dir.path().join("cert.pem");
        let key_path = temp_dir.path().join("key.pem");

        fs::write(&cert_path, cert.pem()).context("write certificate")?;
        fs::write(&key_path, key_pair.serialize_pem()).context("write private key")?;

        Ok(Self { temp_dir, cert_path, key_path })
    }

    /// Generate a mismatched key PEM alongside the certificate.
    pub fn mismatched_key(&self) -> anyhow::Result<PathBuf> {
        let key_pair = KeyPair::generate().context("generate mismatched key")?;
        let path = self.temp_dir.path().join("mismatched_key.pem");
        fs::write(&path, key_pair.serialize_pem()).context("write mismatched key")?;
        Ok(path)
    }
}

// ============================================================================
// Certificate Authority for mTLS E2E Testing
// ============================================================================

/// A test Certificate Authority that can issue server and client certificates.
///
/// This is used for mTLS e2e testing where we need:
/// - A CA certificate for trust stores
/// - Server certificates for the xDS server
/// - Client certificates with SPIFFE URIs for Envoy dataplanes
///
/// All certificates are generated using rcgen (no external PKI required).
pub struct TestCertificateAuthority {
    /// The CA certificate (self-signed)
    ca_cert: rcgen::Certificate,
    /// The CA's key pair for signing issued certificates
    ca_key: KeyPair,
    /// PEM-encoded CA certificate
    ca_cert_pem: String,
    /// Temp directory for CA cert file
    #[allow(dead_code)]
    temp_dir: TempDir,
    /// Path to CA certificate file
    pub ca_cert_path: PathBuf,
}

impl TestCertificateAuthority {
    /// Create a new self-signed Certificate Authority.
    ///
    /// # Arguments
    /// * `common_name` - CN for the CA certificate (e.g., "Flowplane Test CA")
    /// * `valid_for` - How long the CA certificate should be valid
    ///
    /// # Example
    /// ```ignore
    /// let ca = TestCertificateAuthority::new("Test CA", Duration::days(365))?;
    /// ```
    pub fn new(common_name: &str, valid_for: Duration) -> anyhow::Result<Self> {
        let mut params = CertificateParams::new(vec![]).context("create CA certificate params")?;

        // Mark as CA with no intermediate CAs allowed
        params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));

        params.distinguished_name.push(DnType::CommonName, common_name);
        params.distinguished_name.push(DnType::OrganizationName, "Flowplane Test");

        let now = time::OffsetDateTime::now_utc();
        // Use minimal backdating (5 minutes) for clock skew tolerance
        params.not_before = now - Duration::minutes(5);
        params.not_after = now + valid_for;

        // Generate CA key pair
        let ca_key = KeyPair::generate().context("generate CA key pair")?;

        // Self-sign the CA certificate
        let ca_cert = params.self_signed(&ca_key).context("self-sign CA certificate")?;
        let ca_cert_pem = ca_cert.pem();

        // Write CA cert to temp file
        let temp_dir = TempDir::new().context("create temp dir for CA")?;
        let ca_cert_path = temp_dir.path().join("ca.pem");
        fs::write(&ca_cert_path, &ca_cert_pem).context("write CA certificate")?;

        Ok(Self { ca_cert, ca_key, ca_cert_pem, temp_dir, ca_cert_path })
    }

    /// Get the PEM-encoded CA certificate.
    pub fn ca_cert_pem(&self) -> &str {
        &self.ca_cert_pem
    }

    /// Issue a server certificate for xDS TLS.
    ///
    /// # Arguments
    /// * `dns_names` - DNS names for the server (e.g., ["localhost"])
    /// * `valid_for` - How long the certificate should be valid
    ///
    /// # Example
    /// ```ignore
    /// let server_cert = ca.issue_server_cert(&["localhost"], Duration::days(30))?;
    /// ```
    pub fn issue_server_cert(
        &self,
        dns_names: &[&str],
        valid_for: Duration,
    ) -> anyhow::Result<TestCertificateFiles> {
        let dns_names_owned: Vec<String> = dns_names.iter().map(|s| s.to_string()).collect();
        let mut params =
            CertificateParams::new(dns_names_owned).context("create server cert params")?;

        params.is_ca = IsCa::ExplicitNoCa;

        params.distinguished_name.push(DnType::CommonName, "Flowplane xDS Server");
        params.distinguished_name.push(DnType::OrganizationName, "Flowplane Test");

        // Add localhost IP as SAN
        params.subject_alt_names.push(SanType::IpAddress(Ipv4Addr::LOCALHOST.into()));

        let now = time::OffsetDateTime::now_utc();
        // Use minimal backdating (5 minutes) for clock skew tolerance
        params.not_before = now - Duration::minutes(5);
        params.not_after = now + valid_for;

        // Generate server key pair
        let server_key = KeyPair::generate().context("generate server key pair")?;

        // Sign with CA
        let server_cert = params
            .signed_by(&server_key, &self.ca_cert, &self.ca_key)
            .context("sign server certificate")?;

        // Write to temp files
        let temp_dir = TempDir::new().context("create temp dir for server cert")?;
        let cert_path = temp_dir.path().join("server.pem");
        let key_path = temp_dir.path().join("server.key");

        fs::write(&cert_path, server_cert.pem()).context("write server certificate")?;
        fs::write(&key_path, server_key.serialize_pem()).context("write server key")?;

        Ok(TestCertificateFiles { temp_dir, cert_path, key_path })
    }

    /// Issue a client certificate with a SPIFFE URI for mTLS authentication.
    ///
    /// The SPIFFE URI is embedded in the Subject Alternative Name (SAN) extension,
    /// which is how Envoy identifies the client and extracts team information.
    ///
    /// # Arguments
    /// * `spiffe_uri` - Full SPIFFE URI (e.g., "spiffe://flowplane.local/team/eng/proxy/envoy-1")
    /// * `common_name` - CN for the certificate (e.g., "envoy-1")
    /// * `valid_for` - How long the certificate should be valid
    ///
    /// # Example
    /// ```ignore
    /// let client_cert = ca.issue_client_cert(
    ///     "spiffe://flowplane.local/team/engineering/proxy/envoy-1",
    ///     "envoy-1",
    ///     Duration::days(30),
    /// )?;
    /// ```
    pub fn issue_client_cert(
        &self,
        spiffe_uri: &str,
        common_name: &str,
        valid_for: Duration,
    ) -> anyhow::Result<TestCertificateFiles> {
        let mut params = CertificateParams::new(vec![]).context("create client cert params")?;

        params.is_ca = IsCa::ExplicitNoCa;

        params.distinguished_name.push(DnType::CommonName, common_name);
        params.distinguished_name.push(DnType::OrganizationName, "Flowplane Test");

        // Add SPIFFE URI as Subject Alternative Name
        // rcgen 0.13 requires Ia5String for URI SANs
        let uri_ia5 = Ia5String::try_from(spiffe_uri.to_string())
            .map_err(|e| anyhow::anyhow!("Invalid SPIFFE URI: {}", e))?;
        params.subject_alt_names.push(SanType::URI(uri_ia5));

        let now = time::OffsetDateTime::now_utc();
        // Use minimal backdating (5 minutes) for clock skew tolerance
        params.not_before = now - Duration::minutes(5);
        params.not_after = now + valid_for;

        // Generate client key pair
        let client_key = KeyPair::generate().context("generate client key pair")?;

        // Sign with CA
        let client_cert = params
            .signed_by(&client_key, &self.ca_cert, &self.ca_key)
            .context("sign client certificate")?;

        // Write to temp files
        let temp_dir = TempDir::new().context("create temp dir for client cert")?;
        let cert_path = temp_dir.path().join("client.pem");
        let key_path = temp_dir.path().join("client.key");

        fs::write(&cert_path, client_cert.pem()).context("write client certificate")?;
        fs::write(&key_path, client_key.serialize_pem()).context("write client key")?;

        Ok(TestCertificateFiles { temp_dir, cert_path, key_path })
    }

    /// Build a SPIFFE URI for a team and proxy.
    ///
    /// # Arguments
    /// * `trust_domain` - The SPIFFE trust domain (e.g., "flowplane.local")
    /// * `team` - Team name
    /// * `proxy_id` - Proxy/dataplane identifier
    ///
    /// # Returns
    /// A properly formatted SPIFFE URI: `spiffe://{trust_domain}/team/{team}/proxy/{proxy_id}`
    ///
    /// # Errors
    /// Returns error if any component contains path separators ('/')
    /// which could lead to team name injection attacks.
    pub fn build_spiffe_uri(
        trust_domain: &str,
        team: &str,
        proxy_id: &str,
    ) -> anyhow::Result<String> {
        // Security: Validate inputs don't contain path separators
        // This prevents team name injection via malformed inputs
        if trust_domain.contains('/') {
            anyhow::bail!("Trust domain cannot contain '/' (got: {})", trust_domain);
        }
        if team.contains('/') {
            anyhow::bail!("Team name cannot contain '/' (got: {})", team);
        }
        if proxy_id.contains('/') {
            anyhow::bail!("Proxy ID cannot contain '/' (got: {})", proxy_id);
        }

        // Additional validation: no empty components
        if trust_domain.is_empty() {
            anyhow::bail!("Trust domain cannot be empty");
        }
        if team.is_empty() {
            anyhow::bail!("Team name cannot be empty");
        }
        if proxy_id.is_empty() {
            anyhow::bail!("Proxy ID cannot be empty");
        }

        Ok(format!("spiffe://{}/team/{}/proxy/{}", trust_domain, team, proxy_id))
    }

    /// Parse team name from a SPIFFE URI.
    ///
    /// Expected format: `spiffe://{trust_domain}/team/{team}/proxy/{proxy_id}`
    ///
    /// # Returns
    /// The team name if the URI matches the expected format, None otherwise.
    pub fn parse_team_from_spiffe_uri(uri: &str) -> Option<String> {
        // Expected format: spiffe://flowplane.local/team/{team}/proxy/{proxy}
        let parts: Vec<&str> = uri.split('/').collect();
        // Parts: ["spiffe:", "", "flowplane.local", "team", "{team}", "proxy", "{proxy}"]
        if parts.len() >= 5 && parts[3] == "team" {
            Some(parts[4].to_string())
        } else {
            None
        }
    }
}
