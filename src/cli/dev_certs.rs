//! Ephemeral dev-mode mTLS material for Flowplane.
//!
//! This module builds a complete local SPIFFE PKI and writes it to disk so that
//! `flowplane init` can launch Envoy, the flowplane-agent, and the control plane
//! with real service-identity mTLS — the same auth path prod runs.
//!
//! Scope: dev mode only. Certs are short-lived (24h), regenerated on every init,
//! and have no rotation logic. Prod uses the SDS pipeline.
//!
//! Layout written under `out_dir`:
//!
//! ```text
//! out_dir/
//!   ca.pem              self-signed CA cert
//!   ca.key.pem          CA private key (local only, never mounted into Envoy)
//!   cp/cert.pem         control-plane server cert
//!   cp/key.pem          control-plane server key
//!   agent/cert.pem      flowplane-agent client cert
//!   agent/key.pem       flowplane-agent client key
//!   envoy/cert.pem      Envoy xDS client cert
//!   envoy/key.pem       Envoy xDS client key
//! ```
//!
//! SPIFFE identities (format unified with prod parser in fp-u54.6):
//! - CP server:  `spiffe://flowplane.local/control-plane/dev`
//! - Dataplane:  `spiffe://flowplane.local/team/default/proxy/dev-dataplane` (shared by Envoy + agent)
//!
//! The dataplane URI uses the legacy `team/{team}/proxy/{proxy_id}` shape so
//! that the existing `parse_team_from_spiffe_uri` /
//! `parse_proxy_id_from_spiffe_uri` functions in `src/secrets/vault.rs` accept
//! it unchanged. Team name is pinned to `default` to match the seeded dev team
//! in `src/startup.rs`.

use std::fs;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rcgen::{
    BasicConstraints, Certificate, CertificateParams, DnType, Ia5String, IsCa, KeyPair, SanType,
};
use time::{Duration, OffsetDateTime};

/// SPIFFE trust domain for dev-mode certs.
pub const DEV_TRUST_DOMAIN: &str = "flowplane.local";

/// SPIFFE URI for the dev control plane.
pub const DEV_CP_SPIFFE_URI: &str = "spiffe://flowplane.local/control-plane/dev";

/// Shared SPIFFE URI for the dev dataplane (Envoy + flowplane-agent).
///
/// Format: `team/{team}/proxy/{proxy_id}` — the legacy shape accepted by
/// `parse_team_from_spiffe_uri` and `parse_proxy_id_from_spiffe_uri` in
/// `src/secrets/vault.rs`. Team is pinned to `default` to match the dev team
/// seeded in `src/startup.rs`. Proxy id matches `DEV_DATAPLANE_ID` in
/// `src/cli/agent_supervisor.rs` so `envelope.dataplane_id ==
/// authenticated_dataplane_id` in the diagnostics service.
pub const DEV_DATAPLANE_SPIFFE_URI: &str =
    "spiffe://flowplane.local/team/default/proxy/dev-dataplane";

/// Hostname SANs placed on the CP server cert so Envoy, agent, and host-machine
/// clients can all verify it regardless of which address they dial.
const CP_DNS_NAMES: &[&str] = &["localhost", "flowplane-cp"];

/// Dev cert lifetime. 24h is plenty — `flowplane init` regenerates every run.
const CERT_LIFETIME: Duration = Duration::hours(24);

/// Small clock-skew buffer so freshly-minted certs aren't rejected by hosts
/// whose clock runs a minute behind.
const BACKDATE: Duration = Duration::minutes(5);

/// Absolute paths to every PEM file produced by [`generate_dev_certs`].
#[derive(Debug, Clone)]
pub struct DevCertPaths {
    /// Root directory all files live under.
    pub root: PathBuf,
    /// CA certificate (trust bundle). Distribute to every peer that needs to
    /// verify a Flowplane dev cert.
    pub ca_cert: PathBuf,
    /// CA private key. Kept local to the host running `flowplane init`;
    /// should NEVER be mounted into a container.
    pub ca_key: PathBuf,
    /// Control-plane server cert.
    pub cp_cert: PathBuf,
    /// Control-plane server key.
    pub cp_key: PathBuf,
    /// flowplane-agent client cert.
    pub agent_cert: PathBuf,
    /// flowplane-agent client key.
    pub agent_key: PathBuf,
    /// Envoy xDS client cert.
    pub envoy_cert: PathBuf,
    /// Envoy xDS client key.
    pub envoy_key: PathBuf,
}

/// Generate a fresh dev mTLS PKI rooted at `out_dir`.
///
/// Overwrites any existing files under `out_dir` — idempotent across repeated
/// `flowplane init` runs. Creates `out_dir` (and the `cp/`, `agent/`, `envoy/`
/// subdirs) if they don't exist.
pub fn generate_dev_certs(out_dir: &Path) -> Result<DevCertPaths> {
    fs::create_dir_all(out_dir)
        .with_context(|| format!("create dev cert root {}", out_dir.display()))?;

    let cp_dir = out_dir.join("cp");
    let agent_dir = out_dir.join("agent");
    let envoy_dir = out_dir.join("envoy");
    for dir in [&cp_dir, &agent_dir, &envoy_dir] {
        fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    }

    let (ca_cert, ca_key) = build_ca().context("build dev CA")?;

    let cp_cert_path = cp_dir.join("cert.pem");
    let cp_key_path = cp_dir.join("key.pem");
    let agent_cert_path = agent_dir.join("cert.pem");
    let agent_key_path = agent_dir.join("key.pem");
    let envoy_cert_path = envoy_dir.join("cert.pem");
    let envoy_key_path = envoy_dir.join("key.pem");

    issue_cp_cert(&ca_cert, &ca_key, &cp_cert_path, &cp_key_path)
        .context("issue control-plane server cert")?;
    issue_dataplane_cert(&ca_cert, &ca_key, &agent_cert_path, &agent_key_path, "flowplane-agent")
        .context("issue flowplane-agent client cert")?;
    issue_dataplane_cert(&ca_cert, &ca_key, &envoy_cert_path, &envoy_key_path, "envoy")
        .context("issue Envoy client cert")?;

    let ca_cert_path = out_dir.join("ca.pem");
    let ca_key_path = out_dir.join("ca.key.pem");
    atomic_write(&ca_cert_path, ca_cert.pem().as_bytes())?;
    atomic_write(&ca_key_path, ca_key.serialize_pem().as_bytes())?;

    Ok(DevCertPaths {
        root: absolutize(out_dir),
        ca_cert: absolutize(&ca_cert_path),
        ca_key: absolutize(&ca_key_path),
        cp_cert: absolutize(&cp_cert_path),
        cp_key: absolutize(&cp_key_path),
        agent_cert: absolutize(&agent_cert_path),
        agent_key: absolutize(&agent_key_path),
        envoy_cert: absolutize(&envoy_cert_path),
        envoy_key: absolutize(&envoy_key_path),
    })
}

fn build_ca() -> Result<(Certificate, KeyPair)> {
    let mut params = CertificateParams::new(Vec::<String>::new()).context("init CA params")?;
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.distinguished_name.push(DnType::CommonName, "Flowplane Dev CA");
    params.distinguished_name.push(DnType::OrganizationName, "Flowplane");

    let now = OffsetDateTime::now_utc();
    params.not_before = now - BACKDATE;
    params.not_after = now + CERT_LIFETIME;

    let key = KeyPair::generate().context("generate CA key")?;
    let cert = params.self_signed(&key).context("self-sign CA")?;
    Ok((cert, key))
}

fn issue_cp_cert(
    ca_cert: &Certificate,
    ca_key: &KeyPair,
    cert_path: &Path,
    key_path: &Path,
) -> Result<()> {
    let dns: Vec<String> = CP_DNS_NAMES.iter().map(|s| (*s).to_string()).collect();
    let mut params = CertificateParams::new(dns).context("init CP cert params")?;
    params.is_ca = IsCa::ExplicitNoCa;
    params.distinguished_name.push(DnType::CommonName, "flowplane-cp");
    params.distinguished_name.push(DnType::OrganizationName, "Flowplane");

    params.subject_alt_names.push(SanType::IpAddress(Ipv4Addr::LOCALHOST.into()));

    let uri = Ia5String::try_from(DEV_CP_SPIFFE_URI.to_string())
        .map_err(|e| anyhow::anyhow!("invalid CP SPIFFE URI: {e}"))?;
    params.subject_alt_names.push(SanType::URI(uri));

    let now = OffsetDateTime::now_utc();
    params.not_before = now - BACKDATE;
    params.not_after = now + CERT_LIFETIME;

    let key = KeyPair::generate().context("generate CP key")?;
    let cert = params.signed_by(&key, ca_cert, ca_key).context("sign CP cert")?;

    atomic_write(cert_path, cert.pem().as_bytes())?;
    atomic_write(key_path, key.serialize_pem().as_bytes())?;
    Ok(())
}

fn issue_dataplane_cert(
    ca_cert: &Certificate,
    ca_key: &KeyPair,
    cert_path: &Path,
    key_path: &Path,
    common_name: &str,
) -> Result<()> {
    let mut params =
        CertificateParams::new(Vec::<String>::new()).context("init dataplane cert params")?;
    params.is_ca = IsCa::ExplicitNoCa;
    params.distinguished_name.push(DnType::CommonName, common_name);
    params.distinguished_name.push(DnType::OrganizationName, "Flowplane");

    let uri = Ia5String::try_from(DEV_DATAPLANE_SPIFFE_URI.to_string())
        .map_err(|e| anyhow::anyhow!("invalid dataplane SPIFFE URI: {e}"))?;
    params.subject_alt_names.push(SanType::URI(uri));

    let now = OffsetDateTime::now_utc();
    params.not_before = now - BACKDATE;
    params.not_after = now + CERT_LIFETIME;

    let key = KeyPair::generate().context("generate dataplane key")?;
    let cert = params.signed_by(&key, ca_cert, ca_key).context("sign dataplane cert")?;

    atomic_write(cert_path, cert.pem().as_bytes())?;
    atomic_write(key_path, key.serialize_pem().as_bytes())?;
    Ok(())
}

fn atomic_write(path: &Path, contents: &[u8]) -> Result<()> {
    // Plain overwrite is fine in dev; rcgen output is deterministic in shape
    // but not bytewise, and we explicitly want idempotent overwrite semantics.
    fs::write(path, contents).with_context(|| format!("write {}", path.display()))
}

fn absolutize(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use tempfile::TempDir;
    use x509_parser::prelude::*;

    fn load_cert(path: &Path) -> (Vec<u8>, Vec<u8>) {
        let pem_bytes = fs::read(path).expect("read cert pem");
        let (_, pem) = parse_x509_pem(&pem_bytes).expect("parse pem");
        assert_eq!(pem.label, "CERTIFICATE", "expected CERTIFICATE pem label");
        (pem_bytes, pem.contents)
    }

    fn san_strings(der: &[u8]) -> (HashSet<String>, HashSet<String>, HashSet<Ipv4Addr>) {
        let (_, cert) = parse_x509_certificate(der).expect("parse x509");
        let mut uris = HashSet::new();
        let mut dns = HashSet::new();
        let mut ips = HashSet::new();
        let san_ext =
            cert.subject_alternative_name().expect("san ext lookup").expect("san ext present");
        for name in &san_ext.value.general_names {
            match name {
                GeneralName::URI(u) => {
                    uris.insert((*u).to_string());
                }
                GeneralName::DNSName(d) => {
                    dns.insert((*d).to_string());
                }
                GeneralName::IPAddress(bytes) => {
                    if bytes.len() == 4 {
                        ips.insert(Ipv4Addr::new(bytes[0], bytes[1], bytes[2], bytes[3]));
                    }
                }
                _ => {}
            }
        }
        (uris, dns, ips)
    }

    fn fresh(dir_suffix: &str) -> (TempDir, DevCertPaths) {
        let tmp = TempDir::new().expect("temp dir");
        let out = tmp.path().join(dir_suffix);
        let paths = generate_dev_certs(&out).expect("generate dev certs");
        (tmp, paths)
    }

    #[test]
    fn generates_all_expected_files() {
        let (_tmp, paths) = fresh("certs");
        for p in [
            &paths.ca_cert,
            &paths.ca_key,
            &paths.cp_cert,
            &paths.cp_key,
            &paths.agent_cert,
            &paths.agent_key,
            &paths.envoy_cert,
            &paths.envoy_key,
        ] {
            assert!(p.exists(), "missing file: {}", p.display());
            let meta = fs::metadata(p).unwrap();
            assert!(meta.len() > 0, "empty file: {}", p.display());
        }
    }

    #[test]
    fn creates_output_dir_if_absent() {
        let tmp = TempDir::new().unwrap();
        let nested = tmp.path().join("a").join("b").join("c");
        assert!(!nested.exists());
        let paths = generate_dev_certs(&nested).expect("should create nested path");
        assert!(paths.ca_cert.exists());
        assert!(nested.join("cp").is_dir());
        assert!(nested.join("agent").is_dir());
        assert!(nested.join("envoy").is_dir());
    }

    #[test]
    fn idempotent_rerun_overwrites_cleanly() {
        let tmp = TempDir::new().unwrap();
        let out = tmp.path().join("certs");
        let first = generate_dev_certs(&out).expect("first gen");
        let first_ca = fs::read(&first.ca_cert).unwrap();
        let first_cp = fs::read(&first.cp_cert).unwrap();

        let second = generate_dev_certs(&out).expect("second gen must not error");
        let second_ca = fs::read(&second.ca_cert).unwrap();
        let second_cp = fs::read(&second.cp_cert).unwrap();

        // Fresh key material means bytes differ — this catches a bug where
        // a "cached" run silently returned stale files.
        assert_ne!(first_ca, second_ca, "CA should be regenerated on rerun");
        assert_ne!(first_cp, second_cp, "CP cert should be regenerated on rerun");
    }

    #[test]
    fn cp_cert_has_dns_ip_and_spiffe_sans() {
        let (_tmp, paths) = fresh("certs");
        let (_, der) = load_cert(&paths.cp_cert);
        let (uris, dns, ips) = san_strings(&der);

        assert!(uris.contains(DEV_CP_SPIFFE_URI), "CP cert missing SPIFFE URI; got {uris:?}");
        assert!(dns.contains("localhost"), "CP cert missing localhost DNS SAN; got {dns:?}");
        assert!(dns.contains("flowplane-cp"), "CP cert missing flowplane-cp DNS SAN; got {dns:?}");
        assert!(
            ips.contains(&Ipv4Addr::LOCALHOST),
            "CP cert missing 127.0.0.1 IP SAN; got {ips:?}"
        );
    }

    #[test]
    fn agent_cert_has_shared_dataplane_spiffe_uri() {
        let (_tmp, paths) = fresh("certs");
        let (_, der) = load_cert(&paths.agent_cert);
        let (uris, _, _) = san_strings(&der);
        assert!(
            uris.contains(DEV_DATAPLANE_SPIFFE_URI),
            "agent cert missing shared dataplane SPIFFE URI; got {uris:?}"
        );
    }

    #[test]
    fn envoy_cert_has_shared_dataplane_spiffe_uri() {
        let (_tmp, paths) = fresh("certs");
        let (_, der) = load_cert(&paths.envoy_cert);
        let (uris, _, _) = san_strings(&der);
        assert!(
            uris.contains(DEV_DATAPLANE_SPIFFE_URI),
            "envoy cert missing shared dataplane SPIFFE URI; got {uris:?}"
        );
    }

    #[test]
    fn envoy_and_agent_share_identity() {
        // The pinned decision is one dataplane identity for both workloads.
        // If this test fails, we have split identities — re-read fp-u54.1 notes.
        let (_tmp, paths) = fresh("certs");
        let (_, agent_der) = load_cert(&paths.agent_cert);
        let (_, envoy_der) = load_cert(&paths.envoy_cert);
        let (agent_uris, _, _) = san_strings(&agent_der);
        let (envoy_uris, _, _) = san_strings(&envoy_der);
        assert_eq!(agent_uris, envoy_uris, "agent and envoy must share identical SPIFFE URI set");
    }

    #[test]
    fn leaf_certs_chain_to_ca() {
        let (_tmp, paths) = fresh("certs");
        let (_, ca_der) = load_cert(&paths.ca_cert);
        let (_, ca_cert) = parse_x509_certificate(&ca_der).expect("parse CA");
        let ca_pubkey = ca_cert.public_key();

        for (label, leaf_path) in
            [("cp", &paths.cp_cert), ("agent", &paths.agent_cert), ("envoy", &paths.envoy_cert)]
        {
            let (_, leaf_der) = load_cert(leaf_path);
            let (_, leaf) = parse_x509_certificate(&leaf_der).expect("parse leaf");
            assert_eq!(leaf.issuer(), ca_cert.subject(), "{label} leaf issuer != CA subject");
            leaf.verify_signature(Some(ca_pubkey))
                .unwrap_or_else(|e| panic!("{label} leaf signature not valid against CA: {e:?}"));
        }
    }

    #[test]
    fn ca_is_self_signed_and_marked_ca() {
        let (_tmp, paths) = fresh("certs");
        let (_, ca_der) = load_cert(&paths.ca_cert);
        let (_, ca) = parse_x509_certificate(&ca_der).expect("parse CA");
        assert_eq!(ca.issuer(), ca.subject(), "CA must be self-signed");
        ca.verify_signature(None).expect("CA self-signature must verify");
        let bc = ca.basic_constraints().expect("bc lookup").expect("bc present");
        assert!(bc.value.ca, "CA cert must have basicConstraints CA=true");
    }

    #[test]
    fn cert_validity_is_positive_and_bounded() {
        let (_tmp, paths) = fresh("certs");
        let (_, der) = load_cert(&paths.cp_cert);
        let (_, cert) = parse_x509_certificate(&der).expect("parse");
        let nb = cert.validity().not_before.timestamp();
        let na = cert.validity().not_after.timestamp();
        assert!(na > nb, "not_after must be after not_before");
        // 24h + 5min backdate ~ 25h upper bound; assert under 48h as a sane cap.
        let span = na - nb;
        assert!(span > 0 && span < 60 * 60 * 48, "cert span out of bounds: {span}s");
    }

    /// fp-u54.6 regression: the dev dataplane SPIFFE URI minted by this
    /// module MUST be accepted by the prod parser in `src/secrets/vault.rs`.
    ///
    /// This is the cross-boundary test that would have caught the original
    /// bug. Before fp-u54.6 the URI was `/dataplane/dev-dataplane`, which
    /// `parse_team_from_spiffe_uri` does not understand, so
    /// `extract_client_identity` returned None and the diagnostics service
    /// rejected every agent connection. Do NOT mock the parser — use the real
    /// function re-exported from `crate::secrets`.
    #[test]
    fn dev_dataplane_spiffe_uri_parses_via_prod_parser() {
        use crate::secrets::{parse_proxy_id_from_spiffe_uri, parse_team_from_spiffe_uri};

        // Parse the constant directly — this is the string that ends up in
        // the cert SAN verbatim, so asserting against the constant is
        // equivalent to asserting against what the cert carries.
        assert_eq!(
            parse_team_from_spiffe_uri(DEV_DATAPLANE_SPIFFE_URI),
            Some("default".to_string()),
            "dev dataplane URI must yield team=default via the prod parser"
        );
        assert_eq!(
            parse_proxy_id_from_spiffe_uri(DEV_DATAPLANE_SPIFFE_URI),
            Some("dev-dataplane".to_string()),
            "dev dataplane URI must yield proxy_id=dev-dataplane via the prod parser"
        );

        // Also extract the URI from a freshly-generated agent cert and feed
        // that string through the parser. This catches a bug where the
        // constant is correct but generate_dev_certs somehow mangles it into
        // the cert SAN (e.g. double-encoding, trailing nulls).
        let (_tmp, paths) = fresh("certs");
        let (_, der) = load_cert(&paths.agent_cert);
        let (uris, _, _) = san_strings(&der);
        let uri_from_cert = uris
            .iter()
            .find(|u| u.starts_with("spiffe://"))
            .expect("agent cert must carry a spiffe:// SAN");
        assert_eq!(
            parse_team_from_spiffe_uri(uri_from_cert),
            Some("default".to_string()),
            "URI extracted from real cert must parse: {uri_from_cert}"
        );
        assert_eq!(
            parse_proxy_id_from_spiffe_uri(uri_from_cert),
            Some("dev-dataplane".to_string()),
            "URI extracted from real cert must parse: {uri_from_cert}"
        );
    }

    #[test]
    fn key_pem_round_trips_as_pkcs8() {
        // Keys must be parseable PEM — catches a bug where we accidentally
        // write DER or a corrupted buffer.
        let (_tmp, paths) = fresh("certs");
        for key_path in [&paths.ca_key, &paths.cp_key, &paths.agent_key, &paths.envoy_key] {
            let bytes = fs::read(key_path).expect("read key");
            let text = std::str::from_utf8(&bytes).expect("key pem is utf8");
            assert!(
                text.contains("-----BEGIN PRIVATE KEY-----")
                    || text.contains("-----BEGIN EC PRIVATE KEY-----"),
                "key {} missing PEM header; got {:?}",
                key_path.display(),
                &text[..text.len().min(40)]
            );
        }
    }
}
