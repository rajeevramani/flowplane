//! Dataplane + proxy-certificate services (S5.4). REST exposure lands with S6; the xDS
//! mTLS path and tests drive these directly. Same contract as every service: one
//! transaction holding the row change, its outbox event, and its audit entry.

use crate::authz::{check_resource_access, Decision, PrincipalCtx};
use crate::services::{actor_of, deny_to_error, record_authz_denial, trace_context_json};
use fp_domain::authz::{Action, Resource, TeamRef};
use fp_domain::dataplane::{validate_spiffe_uri, Dataplane, ProxyCertificate};
use fp_domain::event::{DomainEvent, EventScope};
use fp_domain::{validate_name, DomainError, DomainResult, RequestId, TeamStatsOverview, UserId};
use fp_storage::repos::{audit, dataplanes};
use openssl::asn1::Asn1Time;
use openssl::bn::BigNum;
use openssl::hash::MessageDigest;
use openssl::pkey::PKey;
use openssl::rsa::Rsa;
use openssl::x509::extension::{BasicConstraints, ExtendedKeyUsage, KeyUsage};
use openssl::x509::{X509NameBuilder, X509};
use sqlx::PgPool;
use std::path::PathBuf;

async fn authorize(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    resource: Resource,
    action: Action,
    team: TeamRef,
    request_id: RequestId,
) -> DomainResult<()> {
    match check_resource_access(ctx, resource, action, Some(team)) {
        Decision::Allow(_) => Ok(()),
        Decision::Deny(reason) => {
            record_authz_denial(pool, ctx, request_id, resource, action, Some(team), reason).await;
            Err(deny_to_error(resource, action, reason))
        }
    }
}

pub async fn create_dataplane(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    description: &str,
    request_id: RequestId,
) -> DomainResult<Dataplane> {
    authorize(
        pool,
        ctx,
        Resource::Dataplanes,
        Action::Create,
        team,
        request_id,
    )
    .await?;
    validate_name(name)?;
    crate::services::quota::check_team_resource_quota(pool, team.id, Resource::Dataplanes).await?;
    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("create dataplane: begin"))?;
    let dataplane = dataplanes::create_dataplane(&mut tx, team, name, description).await?;
    fp_storage::outbox::append(
        &mut tx,
        &DomainEvent::DataplaneCreated {
            dataplane_id: dataplane.id.as_uuid(),
            name: name.into(),
        },
        EventScope {
            org_id: Some(team.org_id),
            team_id: Some(team.id),
        },
        trace_context_json(),
    )
    .await?;
    audit::record_in_tx(
        &mut tx,
        &mutation_audit(
            ctx,
            request_id,
            team,
            "dataplane.create",
            &format!("dataplanes/{name}"),
        ),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("create dataplane: commit"))?;
    Ok(dataplane)
}

pub async fn get_dataplane(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    request_id: RequestId,
) -> DomainResult<Dataplane> {
    authorize(
        pool,
        ctx,
        Resource::Dataplanes,
        Action::Read,
        team,
        request_id,
    )
    .await?;
    dataplanes::get_dataplane(pool, team.id, name)
        .await?
        .ok_or_else(|| fp_domain::DomainError::not_found("dataplane", name))
}

pub async fn list_dataplanes(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    limit: i64,
    offset: i64,
    request_id: RequestId,
) -> DomainResult<(Vec<Dataplane>, i64)> {
    authorize(
        pool,
        ctx,
        Resource::Dataplanes,
        Action::Read,
        team,
        request_id,
    )
    .await?;
    dataplanes::list_dataplanes(pool, team.id, limit, offset).await
}

pub async fn list_certificates(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    request_id: RequestId,
) -> DomainResult<Vec<ProxyCertificate>> {
    authorize(
        pool,
        ctx,
        Resource::ProxyCertificates,
        Action::Read,
        team,
        request_id,
    )
    .await?;
    dataplanes::list_certificates(pool, team.id).await
}

/// What gets registered for a dataplane's certificate (the issued material's metadata;
/// private keys never reach the control plane).
#[derive(Debug, Clone)]
pub struct CertificateRegistration<'a> {
    pub dataplane: &'a str,
    pub spiffe_uri: &'a str,
    pub serial_number: &'a str,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

/// Request to issue a new dataplane client certificate from the configured Flowplane CA.
#[derive(Debug, Clone)]
pub struct CertificateIssueRequest<'a> {
    pub dataplane: &'a str,
    pub ttl_hours: i64,
}

/// One-time issue response. The private key is deliberately not stored by Flowplane; callers
/// must write it to their dataplane secret store immediately.
#[derive(Debug, Clone)]
pub struct IssuedProxyCertificate {
    pub certificate: ProxyCertificate,
    pub certificate_pem: String,
    pub private_key_pem: String,
    pub ca_certificate_pem: String,
}

/// Register an issued certificate against a dataplane. The full SPIFFE URI becomes the
/// mTLS binding key; the cert's own team/proxy segments are never trusted at runtime.
pub async fn register_certificate(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    registration: CertificateRegistration<'_>,
    request_id: RequestId,
) -> DomainResult<ProxyCertificate> {
    authorize(
        pool,
        ctx,
        Resource::ProxyCertificates,
        Action::Create,
        team,
        request_id,
    )
    .await?;
    validate_spiffe_uri(registration.spiffe_uri)?;
    if registration.serial_number.is_empty() || registration.serial_number.len() > 128 {
        return Err(fp_domain::DomainError::validation(
            "certificate serial number must be 1..=128 characters",
        ));
    }
    if registration.expires_at <= chrono::Utc::now() {
        return Err(fp_domain::DomainError::validation(
            "certificate expiry must be in the future",
        ));
    }
    let dataplane = dataplanes::get_dataplane(pool, team.id, registration.dataplane)
        .await?
        .ok_or_else(|| fp_domain::DomainError::not_found("dataplane", registration.dataplane))?;
    let issued_by: Option<UserId> = match ctx {
        PrincipalCtx::User { user_id, .. } => Some(*user_id),
        PrincipalCtx::Agent { .. } => None,
    };

    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("register certificate: begin"))?;
    let cert = dataplanes::register_certificate(
        &mut tx,
        team.id,
        dataplane.id,
        registration.spiffe_uri,
        registration.serial_number,
        registration.expires_at,
        issued_by,
    )
    .await?;
    fp_storage::outbox::append(
        &mut tx,
        &DomainEvent::ProxyCertificateRegistered {
            certificate_id: cert.id.as_uuid(),
            spiffe_uri: registration.spiffe_uri.into(),
        },
        EventScope {
            org_id: Some(team.org_id),
            team_id: Some(team.id),
        },
        trace_context_json(),
    )
    .await?;
    audit::record_in_tx(
        &mut tx,
        &mutation_audit(
            ctx,
            request_id,
            team,
            "proxy-certificate.register",
            &format!("proxy-certificates/{}", registration.serial_number),
        ),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("register certificate: commit"))?;
    Ok(cert)
}

/// Issue a dataplane client certificate and register its SPIFFE URI binding. The CA comes
/// from FLOWPLANE_CERT_ISSUER_CA_CERT_PATH / FLOWPLANE_CERT_ISSUER_CA_KEY_PATH.
pub async fn issue_certificate(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    request: CertificateIssueRequest<'_>,
    request_id: RequestId,
) -> DomainResult<IssuedProxyCertificate> {
    authorize(
        pool,
        ctx,
        Resource::ProxyCertificates,
        Action::Create,
        team,
        request_id,
    )
    .await?;

    if !(1..=8760).contains(&request.ttl_hours) {
        return Err(DomainError::validation(
            "certificate ttl_hours must be between 1 and 8760",
        ));
    }

    let dataplane = dataplanes::get_dataplane(pool, team.id, request.dataplane)
        .await?
        .ok_or_else(|| DomainError::not_found("dataplane", request.dataplane))?;
    let issuer = CertificateIssuer::load()?;
    let spiffe_uri = format!(
        "spiffe://{}/org/{}/team/{}/proxy/{}",
        issuer.trust_domain,
        team.org_id.as_uuid(),
        team.id.as_uuid(),
        dataplane.id.as_uuid()
    );
    let serial_number = uuid::Uuid::now_v7().simple().to_string();
    let expires_at = chrono::Utc::now() + chrono::Duration::hours(request.ttl_hours);
    let issued = issuer.issue(&dataplane.name, &spiffe_uri, &serial_number, expires_at)?;

    let issued_by: Option<UserId> = match ctx {
        PrincipalCtx::User { user_id, .. } => Some(*user_id),
        PrincipalCtx::Agent { .. } => None,
    };
    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("issue certificate: begin"))?;
    let cert = dataplanes::register_certificate(
        &mut tx,
        team.id,
        dataplane.id,
        &spiffe_uri,
        &serial_number,
        expires_at,
        issued_by,
    )
    .await?;
    fp_storage::outbox::append(
        &mut tx,
        &DomainEvent::ProxyCertificateRegistered {
            certificate_id: cert.id.as_uuid(),
            spiffe_uri,
        },
        EventScope {
            org_id: Some(team.org_id),
            team_id: Some(team.id),
        },
        trace_context_json(),
    )
    .await?;
    audit::record_in_tx(
        &mut tx,
        &mutation_audit(
            ctx,
            request_id,
            team,
            "proxy-certificate.issue",
            &format!("proxy-certificates/{serial_number}"),
        ),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("issue certificate: commit"))?;

    Ok(IssuedProxyCertificate {
        certificate: cert,
        certificate_pem: issued.certificate_pem,
        private_key_pem: issued.private_key_pem,
        ca_certificate_pem: issuer.ca_certificate_pem,
    })
}

/// Revoke a certificate. The emitted event terminates any live xDS stream authenticated by
/// this certificate (fp-xds revocation bus); reconnects fail at the registry lookup.
pub async fn revoke_certificate(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    serial_number: &str,
    reason: &str,
    request_id: RequestId,
) -> DomainResult<ProxyCertificate> {
    authorize(
        pool,
        ctx,
        Resource::ProxyCertificates,
        Action::Update,
        team,
        request_id,
    )
    .await?;
    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("revoke certificate: begin"))?;
    let cert = dataplanes::revoke_certificate(&mut tx, team.id, serial_number, reason).await?;
    fp_storage::outbox::append(
        &mut tx,
        &DomainEvent::ProxyCertificateRevoked {
            certificate_id: cert.id.as_uuid(),
            spiffe_uri: cert.spiffe_uri.clone(),
        },
        EventScope {
            org_id: Some(team.org_id),
            team_id: Some(team.id),
        },
        trace_context_json(),
    )
    .await?;
    audit::record_in_tx(
        &mut tx,
        &mutation_audit(
            ctx,
            request_id,
            team,
            "proxy-certificate.revoke",
            &format!("proxy-certificates/{serial_number}"),
        ),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("revoke certificate: commit"))?;
    Ok(cert)
}

#[derive(Debug, Clone)]
pub struct DataplaneTelemetry {
    pub idempotency_key: String,
    pub requests_delta: i64,
    pub errors_delta: i64,
    pub warming_failures_delta: i64,
    pub config_verified: bool,
}

pub async fn record_telemetry(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    telemetry: DataplaneTelemetry,
    request_id: RequestId,
) -> DomainResult<Dataplane> {
    authorize(pool, ctx, Resource::Stats, Action::Update, team, request_id).await?;
    validate_idempotency_key(&telemetry.idempotency_key)?;
    // Deliberate audit exemption: telemetry heartbeats are high-frequency derived diagnostics,
    // not operator intent. They are authorized, idempotent, and persisted in dataplane counters;
    // auditing each heartbeat would drown out human/admin changes.
    dataplanes::record_telemetry(
        pool,
        team.id,
        name,
        dataplanes::TelemetryDelta {
            idempotency_key: &telemetry.idempotency_key,
            requests_delta: telemetry.requests_delta,
            errors_delta: telemetry.errors_delta,
            warming_failures_delta: telemetry.warming_failures_delta,
            config_verified: telemetry.config_verified,
        },
    )
    .await
}

fn validate_idempotency_key(key: &str) -> DomainResult<()> {
    if key.is_empty() || key.len() > 200 || key.chars().any(|c| c.is_control() || c == '\0') {
        return Err(DomainError::validation(
            "telemetry idempotency_key must be 1-200 printable characters",
        ));
    }
    Ok(())
}

pub async fn stats_overview(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    request_id: RequestId,
) -> DomainResult<TeamStatsOverview> {
    authorize(pool, ctx, Resource::Stats, Action::Read, team, request_id).await?;
    dataplanes::stats_overview(pool, team.id, chrono::Utc::now()).await
}

fn mutation_audit(
    ctx: &PrincipalCtx,
    request_id: RequestId,
    team: TeamRef,
    action: &str,
    resource: &str,
) -> audit::AuditEntry {
    let (actor_type, actor_id) = actor_of(ctx);
    audit::AuditEntry {
        request_id: Some(request_id),
        actor_type,
        actor_id,
        actor_label: String::new(),
        surface: audit::Surface::Rest,
        action: action.into(),
        resource: resource.into(),
        org_id: Some(team.org_id),
        team_id: Some(team.id),
        outcome: audit::Outcome::Success,
        detail: serde_json::json!({}),
    }
}

struct CertificateIssuer {
    ca_certificate_pem: String,
    ca_key_pem: String,
    trust_domain: String,
}

struct IssuedPem {
    certificate_pem: String,
    private_key_pem: String,
}

impl CertificateIssuer {
    fn load() -> DomainResult<Self> {
        let ca_cert_path = required_env_path("FLOWPLANE_CERT_ISSUER_CA_CERT_PATH")?;
        let ca_key_path = required_env_path("FLOWPLANE_CERT_ISSUER_CA_KEY_PATH")?;
        let trust_domain = std::env::var("FLOWPLANE_CERT_ISSUER_TRUST_DOMAIN")
            .unwrap_or_else(|_| "flowplane.local".into());
        validate_trust_domain(&trust_domain)?;
        Ok(Self {
            ca_certificate_pem: read_pem_file(&ca_cert_path, "certificate issuer CA certificate")?,
            ca_key_pem: read_pem_file(&ca_key_path, "certificate issuer CA key")?,
            trust_domain,
        })
    }

    fn issue(
        &self,
        common_name: &str,
        spiffe_uri: &str,
        serial_number: &str,
        expires_at: chrono::DateTime<chrono::Utc>,
    ) -> DomainResult<IssuedPem> {
        let ca_cert = X509::from_pem(self.ca_certificate_pem.as_bytes()).map_err(|e| {
            DomainError::invalid_config(format!(
                "certificate issuer CA certificate is invalid: {e}"
            ))
        })?;
        let ca_key = PKey::private_key_from_pem(self.ca_key_pem.as_bytes()).map_err(|e| {
            DomainError::invalid_config(format!("certificate issuer CA key is invalid: {e}"))
        })?;

        let leaf_key = PKey::from_rsa(
            Rsa::generate(2048)
                .map_err(|e| DomainError::internal(format!("generate certificate key: {e}")))?,
        )
        .map_err(|e| DomainError::internal(format!("prepare certificate key: {e}")))?;

        let mut builder = X509::builder()
            .map_err(|e| DomainError::internal(format!("create certificate builder: {e}")))?;
        builder
            .set_version(2)
            .map_err(|e| DomainError::internal(format!("set certificate version: {e}")))?;
        let serial = BigNum::from_hex_str(serial_number)
            .and_then(|n| n.to_asn1_integer())
            .map_err(|e| DomainError::internal(format!("set certificate serial: {e}")))?;
        builder
            .set_serial_number(&serial)
            .map_err(|e| DomainError::internal(format!("set certificate serial: {e}")))?;

        let mut name = X509NameBuilder::new()
            .map_err(|e| DomainError::internal(format!("create certificate subject: {e}")))?;
        name.append_entry_by_text("CN", common_name)
            .map_err(|e| DomainError::internal(format!("set certificate common name: {e}")))?;
        let name = name.build();
        builder
            .set_subject_name(&name)
            .map_err(|e| DomainError::internal(format!("set certificate subject: {e}")))?;
        builder
            .set_issuer_name(ca_cert.subject_name())
            .map_err(|e| DomainError::internal(format!("set certificate issuer: {e}")))?;
        builder
            .set_pubkey(&leaf_key)
            .map_err(|e| DomainError::internal(format!("set certificate public key: {e}")))?;

        let not_before = Asn1Time::days_from_now(0)
            .map_err(|e| DomainError::internal(format!("set certificate not_before: {e}")))?;
        let not_after = Asn1Time::from_unix(expires_at.timestamp())
            .map_err(|e| DomainError::internal(format!("set certificate not_after: {e}")))?;
        builder
            .set_not_before(&not_before)
            .map_err(|e| DomainError::internal(format!("set certificate not_before: {e}")))?;
        builder
            .set_not_after(&not_after)
            .map_err(|e| DomainError::internal(format!("set certificate not_after: {e}")))?;

        builder
            .append_extension(BasicConstraints::new().critical().build().map_err(|e| {
                DomainError::internal(format!("set certificate basic constraints: {e}"))
            })?)
            .map_err(|e| {
                DomainError::internal(format!("append certificate basic constraints: {e}"))
            })?;
        builder
            .append_extension(
                KeyUsage::new()
                    .digital_signature()
                    .key_encipherment()
                    .build()
                    .map_err(|e| {
                        DomainError::internal(format!("set certificate key usage: {e}"))
                    })?,
            )
            .map_err(|e| DomainError::internal(format!("append certificate key usage: {e}")))?;
        builder
            .append_extension(ExtendedKeyUsage::new().client_auth().build().map_err(|e| {
                DomainError::internal(format!("set certificate extended key usage: {e}"))
            })?)
            .map_err(|e| {
                DomainError::internal(format!("append certificate extended key usage: {e}"))
            })?;
        let san = {
            let context = builder.x509v3_context(Some(&ca_cert), None);
            openssl::x509::extension::SubjectAlternativeName::new()
                .uri(spiffe_uri)
                .build(&context)
                .map_err(|e| DomainError::internal(format!("set certificate SAN: {e}")))?
        };
        builder
            .append_extension(san)
            .map_err(|e| DomainError::internal(format!("append certificate SAN: {e}")))?;
        builder
            .sign(&ca_key, MessageDigest::sha256())
            .map_err(|e| DomainError::internal(format!("sign certificate: {e}")))?;
        let cert = builder.build();
        let certificate_pem = String::from_utf8(
            cert.to_pem()
                .map_err(|e| DomainError::internal(format!("encode certificate PEM: {e}")))?,
        )
        .map_err(|e| DomainError::internal(format!("encode certificate PEM: {e}")))?;
        let private_key_pem = String::from_utf8(
            leaf_key
                .private_key_to_pem_pkcs8()
                .map_err(|e| DomainError::internal(format!("encode private key PEM: {e}")))?,
        )
        .map_err(|e| DomainError::internal(format!("encode private key PEM: {e}")))?;
        Ok(IssuedPem {
            certificate_pem,
            private_key_pem,
        })
    }
}

fn required_env_path(name: &str) -> DomainResult<PathBuf> {
    std::env::var(name).map(PathBuf::from).map_err(|_| {
        DomainError::invalid_config(format!("{name} is not configured")).with_hint(
            "set FLOWPLANE_CERT_ISSUER_CA_CERT_PATH and \
                 FLOWPLANE_CERT_ISSUER_CA_KEY_PATH to enable certificate issuance",
        )
    })
}

fn read_pem_file(path: &std::path::Path, what: &str) -> DomainResult<String> {
    std::fs::read_to_string(path).map_err(|e| {
        DomainError::invalid_config(format!("cannot read {what} {}: {e}", path.display()))
    })
}

fn validate_trust_domain(value: &str) -> DomainResult<()> {
    if value.is_empty()
        || value.len() > 255
        || value
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '/')
    {
        return Err(DomainError::invalid_config(
            "FLOWPLANE_CERT_ISSUER_TRUST_DOMAIN must be a non-empty SPIFFE trust domain",
        ));
    }
    Ok(())
}
