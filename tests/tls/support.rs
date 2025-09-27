use std::{fs, net::Ipv4Addr, path::PathBuf};

use anyhow::Context;
use rcgen::{Certificate, CertificateParams, DnType, SanType, PKCS_ECDSA_P256_SHA256};
use tempfile::TempDir;
use time::Duration;

/// Helper for generating ephemeral certificate files for integration tests.
pub struct TestCertificateFiles {
    temp_dir: TempDir,
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
}

impl TestCertificateFiles {
    /// Generate a localhost certificate valid for the provided duration.
    pub fn localhost(valid_for: Duration) -> anyhow::Result<Self> {
        let mut params = CertificateParams::new(vec!["localhost".into()])
            .context("build certificate params")?;

        params.alg = &PKCS_ECDSA_P256_SHA256;
        params.distinguished_name.push(DnType::CommonName, "Flowplane Test");
        params.distinguished_name.push(DnType::OrganizationName, "Flowplane");

        params
            .subject_alt_names
            .push(SanType::IpAddress(Ipv4Addr::LOCALHOST.into()));

        let now = time::OffsetDateTime::now_utc();
        params.not_before = now - Duration::days(1);
        params.not_after = now + valid_for;

        Self::from_params(params)
    }

    pub fn with_expiration(not_after: time::OffsetDateTime) -> anyhow::Result<Self> {
        let mut params = CertificateParams::new(vec!["localhost".into()])
            .context("build certificate params")?;

        params.alg = &PKCS_ECDSA_P256_SHA256;
        params.distinguished_name.push(DnType::CommonName, "Flowplane Test");
        params.distinguished_name.push(DnType::OrganizationName, "Flowplane");
        params
            .subject_alt_names
            .push(SanType::IpAddress(Ipv4Addr::LOCALHOST.into()));

        params.not_before = time::OffsetDateTime::now_utc() - Duration::days(1);
        params.not_after = not_after;

        Self::from_params(params)
    }

    fn from_params(params: CertificateParams) -> anyhow::Result<Self> {
        let cert = Certificate::from_params(params).context("build certificate")?;
        let temp_dir = TempDir::new().context("create temp dir")?;

        let cert_path = temp_dir.path().join("cert.pem");
        let key_path = temp_dir.path().join("key.pem");

        fs::write(&cert_path, cert.serialize_pem().context("serialize cert")?)
            .context("write certificate")?;
        fs::write(&key_path, cert.serialize_private_key_pem())
            .context("write private key")?;

        Ok(Self { temp_dir, cert_path, key_path })
    }

    /// Generate a mismatched key PEM alongside the certificate.
    pub fn mismatched_key(&self) -> anyhow::Result<PathBuf> {
        let key_pair = rcgen::KeyPair::generate(&PKCS_ECDSA_P256_SHA256)
            .context("generate mismatched key")?;
        let path = self.temp_dir.path().join("mismatched_key.pem");
        fs::write(&path, key_pair.serialize_pem()).context("write mismatched key")?;
        Ok(path)
    }
}
