//! Dataplane + proxy-certificate services (S5.4). REST exposure lands with S6; the xDS
//! mTLS path and tests drive these directly. Same contract as every service: one
//! transaction holding the row change, its outbox event, and its audit entry.

use crate::authz::{check_resource_access, Decision, PrincipalCtx};
use crate::services::{actor_of, deny_to_error, record_authz_denial, trace_context_json};
use fp_domain::authz::{Action, Resource, TeamRef};
use fp_domain::dataplane::{
    canonical_certificate_serial, validate_spiffe_uri, Dataplane, ProxyCertificate,
};
use fp_domain::event::{DomainEvent, EventScope};
use fp_domain::{validate_name, DomainError, DomainResult, RequestId, TeamStatsOverview, UserId};
use fp_storage::repos::{audit, dataplanes};
use openssl::asn1::Asn1Time;
use openssl::bn::BigNum;
use openssl::hash::MessageDigest;
use openssl::pkey::{PKey, Private};
use openssl::rsa::Rsa;
use openssl::stack::Stack;
use openssl::x509::extension::{
    AuthorityKeyIdentifier, BasicConstraints, ExtendedKeyUsage, KeyUsage, SubjectKeyIdentifier,
};
use openssl::x509::store::X509StoreBuilder;
use openssl::x509::verify::X509VerifyFlags;
use openssl::x509::{X509NameBuilder, X509PurposeId, X509StoreContext, X509};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
    list_dataplanes_with_lifecycle(pool, ctx, team, limit, offset, false, request_id).await
}

pub async fn list_dataplanes_with_lifecycle(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    limit: i64,
    offset: i64,
    include_retired: bool,
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
    dataplanes::list_dataplanes_with_lifecycle(pool, team.id, limit, offset, include_retired).await
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

/// Pin the exact presented leaf fingerprint onto one active legacy credential. This internal
/// xDS-authenticator operation has no public endpoint and records its system mutation atomically.
pub async fn pin_legacy_certificate_fingerprint(
    pool: &PgPool,
    spiffe_uri: &str,
    serial_number: &str,
    fingerprint_sha256: &str,
    request_id: RequestId,
) -> DomainResult<ProxyCertificate> {
    validate_spiffe_uri(spiffe_uri)?;
    let serial_number = canonical_certificate_serial(serial_number)?;
    if fingerprint_sha256.len() != 64
        || !fingerprint_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(DomainError::validation(
            "certificate fingerprint must be 64 lowercase hexadecimal characters",
        ));
    }

    let mut tx = pool.begin().await.map_err(crate::services::db_err(
        "pin legacy certificate fingerprint: begin",
    ))?;
    let certificate = dataplanes::pin_legacy_certificate_fingerprint(
        &mut tx,
        spiffe_uri,
        &serial_number,
        fingerprint_sha256,
    )
    .await?;
    let org_id: uuid::Uuid = sqlx::query_scalar("SELECT org_id FROM teams WHERE id = $1")
        .bind(certificate.team_id.as_uuid())
        .fetch_one(&mut *tx)
        .await
        .map_err(crate::services::db_err(
            "pin legacy certificate fingerprint: resolve organization",
        ))?;
    let org_id = fp_domain::OrgId::from(org_id);

    audit::record_in_tx(
        &mut tx,
        &audit::AuditEntry {
            request_id: Some(request_id),
            actor_type: audit::ActorType::System,
            actor_id: None,
            actor_label: "xds-authenticator".into(),
            surface: audit::Surface::Xds,
            action: "proxy_certificate.fingerprint_pin".into(),
            resource: format!("proxy-certificates/{}", certificate.id),
            org_id: Some(org_id),
            team_id: Some(certificate.team_id),
            outcome: audit::Outcome::Success,
            detail: serde_json::json!({
                "certificate_id": certificate.id.as_uuid(),
                "dataplane_id": certificate.dataplane_id.as_uuid(),
                "fingerprint_sha256": fingerprint_sha256,
            }),
        },
    )
    .await?;
    fp_storage::outbox::append(
        &mut tx,
        &DomainEvent::ProxyCertificateFingerprintPinned {
            certificate_id: certificate.id.as_uuid(),
            spiffe_uri: certificate.spiffe_uri.clone(),
            fingerprint_sha256: fingerprint_sha256.into(),
        },
        EventScope {
            org_id: Some(org_id),
            team_id: Some(certificate.team_id),
        },
        trace_context_json(),
    )
    .await?;
    tx.commit().await.map_err(crate::services::db_err(
        "pin legacy certificate fingerprint: commit",
    ))?;
    Ok(certificate)
}

/// Externally issued certificate material. Identity fields are derived only after chain
/// verification; callers cannot assert SPIFFE URI, serial, or validity metadata.
#[derive(Debug, Clone)]
pub struct CertificateRegistration<'a> {
    pub dataplane: &'a str,
    pub certificate_chain_pem: &'a str,
}

#[derive(Debug, Clone)]
pub struct CertificateChainVerifier {
    trust_roots: Option<Arc<Vec<X509>>>,
    unavailable_reason: Option<Arc<str>>,
}

#[derive(Debug)]
struct VerifiedExternalCertificate {
    spiffe_uri: String,
    serial_number: String,
    fingerprint_sha256: String,
    not_before: chrono::DateTime<chrono::Utc>,
    not_after: chrono::DateTime<chrono::Utc>,
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

/// Register externally issued certificate material after verifying it against xDS client trust.
pub async fn register_external_certificate(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    registration: CertificateRegistration<'_>,
    verifier: &CertificateChainVerifier,
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
    let dataplane = dataplanes::get_dataplane(pool, team.id, registration.dataplane)
        .await?
        .ok_or_else(|| fp_domain::DomainError::not_found("dataplane", registration.dataplane))?;
    let verified = verifier.verify(registration.certificate_chain_pem, dataplane.id.as_uuid())?;
    let issued_by: Option<UserId> = match ctx {
        PrincipalCtx::User { user_id, .. } => Some(*user_id),
        PrincipalCtx::Agent { .. } => None,
    };

    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("register certificate: begin"))?;
    dataplanes::lock_certificate_capacity(&mut tx, team.id, dataplane.id).await?;
    let cert = dataplanes::register_certificate(
        &mut tx,
        dataplanes::NewProxyCertificate {
            team_id: team.id,
            dataplane_id: dataplane.id,
            spiffe_uri: &verified.spiffe_uri,
            serial_number: &verified.serial_number,
            fingerprint_sha256: Some(&verified.fingerprint_sha256),
            issued_at: verified.not_before,
            expires_at: verified.not_after,
            issued_by,
        },
    )
    .await?;
    fp_storage::outbox::append(
        &mut tx,
        &DomainEvent::ProxyCertificateRegistered {
            certificate_id: cert.id.as_uuid(),
            spiffe_uri: verified.spiffe_uri,
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
            &format!("proxy-certificates/{}", verified.serial_number),
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
    let serial_number = canonical_certificate_serial(&uuid::Uuid::now_v7().simple().to_string())?;
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
    dataplanes::lock_certificate_capacity(&mut tx, team.id, dataplane.id).await?;
    let cert = dataplanes::register_certificate(
        &mut tx,
        dataplanes::NewProxyCertificate {
            team_id: team.id,
            dataplane_id: dataplane.id,
            spiffe_uri: &spiffe_uri,
            serial_number: &serial_number,
            fingerprint_sha256: Some(&issued.fingerprint_sha256),
            issued_at: chrono::Utc::now(),
            expires_at,
            issued_by,
        },
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
    let serial_number = canonical_certificate_serial(serial_number)?;
    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("revoke certificate: begin"))?;
    let cert = dataplanes::revoke_certificate(&mut tx, team.id, &serial_number, reason).await?;
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

pub async fn retire_dataplane(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    expected_revision: i64,
    reason: &str,
    request_id: RequestId,
) -> DomainResult<Dataplane> {
    authorize(
        pool,
        ctx,
        Resource::Dataplanes,
        Action::Delete,
        team,
        request_id,
    )
    .await?;
    let reason = reason.trim();
    if reason.is_empty() || reason.len() > 500 {
        return Err(DomainError::validation(
            "retirement reason must contain 1..=500 characters",
        ));
    }

    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("retire dataplane: begin"))?;
    let (retired, certificates) =
        dataplanes::retire_dataplane(&mut tx, team.id, name, expected_revision, reason).await?;
    for certificate in &certificates {
        fp_storage::outbox::append(
            &mut tx,
            &DomainEvent::ProxyCertificateRevoked {
                certificate_id: certificate.id.as_uuid(),
                spiffe_uri: certificate.spiffe_uri.clone(),
            },
            EventScope {
                org_id: Some(team.org_id),
                team_id: Some(team.id),
            },
            trace_context_json(),
        )
        .await?;
    }
    fp_storage::outbox::append(
        &mut tx,
        &DomainEvent::DataplaneRetired {
            dataplane_id: retired.id.as_uuid(),
            name: retired.name.clone(),
        },
        EventScope {
            org_id: Some(team.org_id),
            team_id: Some(team.id),
        },
        trace_context_json(),
    )
    .await?;
    let mut audit_entry = mutation_audit(
        ctx,
        request_id,
        team,
        "dataplane.retire",
        &format!("dataplanes/{name}"),
    );
    audit_entry.detail = serde_json::json!({
        "dataplane_id": retired.id.as_uuid(),
        "revoked_certificate_count": certificates.len(),
    });
    audit::record_in_tx(&mut tx, &audit_entry).await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("retire dataplane: commit"))?;
    Ok(retired)
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

impl CertificateChainVerifier {
    pub fn from_trust_root_path(path: Option<&Path>) -> Self {
        let Some(path) = path else {
            return Self {
                trust_roots: None,
                unavailable_reason: Some("xDS client trust roots are not configured".into()),
            };
        };
        let result = std::fs::read(path)
            .map_err(|error| {
                format!(
                    "cannot read xDS client trust roots {}: {error}",
                    path.display()
                )
            })
            .and_then(|pem| {
                X509::stack_from_pem(&pem)
                    .map_err(|error| format!("xDS client trust roots are invalid: {error}"))
            })
            .and_then(|roots| {
                if roots.is_empty() {
                    Err("xDS client trust roots contain no certificates".to_owned())
                } else {
                    Ok(roots)
                }
            });
        match result {
            Ok(roots) => Self {
                trust_roots: Some(Arc::new(roots)),
                unavailable_reason: None,
            },
            Err(reason) => Self {
                trust_roots: None,
                unavailable_reason: Some(reason.into()),
            },
        }
    }

    fn verify(
        &self,
        certificate_chain_pem: &str,
        expected_dataplane_id: uuid::Uuid,
    ) -> DomainResult<VerifiedExternalCertificate> {
        let roots = self.trust_roots.as_ref().ok_or_else(|| {
            DomainError::invalid_config(
                self.unavailable_reason
                    .as_deref()
                    .unwrap_or("xDS client trust roots are unavailable"),
            )
        })?;
        if certificate_chain_pem.contains("PRIVATE KEY") {
            return Err(DomainError::validation(
                "certificate_chain_pem must not contain private key material",
            ));
        }
        let certificates =
            X509::stack_from_pem(certificate_chain_pem.as_bytes()).map_err(|_| {
                DomainError::validation("certificate_chain_pem must contain valid PEM certificates")
            })?;
        let leaf = certificates.first().ok_or_else(|| {
            DomainError::validation("certificate_chain_pem must contain one leaf certificate")
        })?;

        let mut untrusted = Stack::new().map_err(|error| {
            DomainError::internal(format!("prepare certificate chain verification: {error}"))
        })?;
        for intermediate in certificates.iter().skip(1) {
            if !certificate_is_ca(intermediate)? {
                return Err(DomainError::validation(
                    "certificate_chain_pem contains more than one leaf certificate",
                ));
            }
            untrusted.push(intermediate.to_owned()).map_err(|error| {
                DomainError::internal(format!("prepare intermediate certificate: {error}"))
            })?;
        }

        let mut store = X509StoreBuilder::new().map_err(|error| {
            DomainError::internal(format!("create xDS client trust store: {error}"))
        })?;
        store
            .set_flags(X509VerifyFlags::X509_STRICT | X509VerifyFlags::PARTIAL_CHAIN)
            .map_err(|error| {
                DomainError::internal(format!("configure xDS client trust store: {error}"))
            })?;
        store
            .set_purpose(X509PurposeId::SSL_CLIENT)
            .map_err(|error| {
                DomainError::internal(format!("configure xDS client certificate purpose: {error}"))
            })?;
        for root in roots.iter() {
            store.add_cert(root.to_owned()).map_err(|error| {
                DomainError::invalid_config(format!("add xDS client trust root: {error}"))
            })?;
        }
        let store = store.build();
        let mut context = X509StoreContext::new().map_err(|error| {
            DomainError::internal(format!("create certificate verification context: {error}"))
        })?;
        let (verified, verify_error) = context
            .init(&store, leaf, &untrusted, |context| {
                let verified = context.verify_cert()?;
                Ok((verified, context.error()))
            })
            .map_err(|error| {
                DomainError::validation(format!("certificate chain verification failed: {error}"))
            })?;
        if !verified {
            return Err(DomainError::validation(format!(
                "certificate chain is not trusted: {}",
                verify_error.error_string()
            )));
        }

        derive_external_certificate(leaf, expected_dataplane_id)
    }
}

fn certificate_is_ca(certificate: &X509) -> DomainResult<bool> {
    let der = certificate.to_der().map_err(|error| {
        DomainError::validation(format!("encode certificate for validation: {error}"))
    })?;
    let (_, parsed) = x509_parser::parse_x509_certificate(&der)
        .map_err(|_| DomainError::validation("certificate DER is malformed"))?;
    Ok(parsed
        .basic_constraints()
        .map_err(|_| DomainError::validation("certificate Basic Constraints are malformed"))?
        .is_some_and(|constraints| constraints.value.ca))
}

fn derive_external_certificate(
    leaf: &X509,
    expected_dataplane_id: uuid::Uuid,
) -> DomainResult<VerifiedExternalCertificate> {
    let der = leaf
        .to_der()
        .map_err(|error| DomainError::validation(format!("encode leaf certificate: {error}")))?;
    let (_, parsed) = x509_parser::parse_x509_certificate(&der)
        .map_err(|_| DomainError::validation("leaf certificate DER is malformed"))?;
    if parsed
        .basic_constraints()
        .map_err(|_| DomainError::validation("leaf Basic Constraints are malformed"))?
        .is_some_and(|constraints| constraints.value.ca)
    {
        return Err(DomainError::validation(
            "registered certificate must be a leaf",
        ));
    }
    let key_usage = parsed
        .key_usage()
        .map_err(|_| DomainError::validation("leaf Key Usage is malformed"))?
        .ok_or_else(|| DomainError::validation("leaf must declare Key Usage"))?;
    if !key_usage.value.digital_signature() {
        return Err(DomainError::validation(
            "leaf Key Usage must allow digitalSignature",
        ));
    }
    let eku = parsed
        .extended_key_usage()
        .map_err(|_| DomainError::validation("leaf Extended Key Usage is malformed"))?
        .ok_or_else(|| DomainError::validation("leaf must declare Extended Key Usage"))?;
    if !(eku.value.any || eku.value.client_auth) {
        return Err(DomainError::validation(
            "leaf Extended Key Usage must allow clientAuth",
        ));
    }
    let san = parsed
        .subject_alternative_name()
        .map_err(|_| DomainError::validation("leaf Subject Alternative Name is malformed"))?
        .ok_or_else(|| DomainError::validation("leaf must contain a SPIFFE URI SAN"))?;
    let uris = san
        .value
        .general_names
        .iter()
        .filter_map(|name| match name {
            x509_parser::extensions::GeneralName::URI(uri) => Some(*uri),
            _ => None,
        })
        .collect::<Vec<_>>();
    if uris.len() != 1 {
        return Err(DomainError::validation(
            "leaf must contain exactly one SPIFFE URI SAN",
        ));
    }
    let spiffe_uri = uris[0];
    validate_spiffe_uri(spiffe_uri)?;
    let (_, dataplane_segment) = spiffe_uri.rsplit_once("/proxy/").ok_or_else(|| {
        DomainError::validation("leaf SPIFFE URI must end in /proxy/{dataplane_uuid}")
    })?;
    let dataplane_id = uuid::Uuid::parse_str(dataplane_segment).map_err(|_| {
        DomainError::validation("leaf SPIFFE URI proxy segment must be a dataplane UUID")
    })?;
    if dataplane_id != expected_dataplane_id {
        return Err(DomainError::validation(
            "leaf SPIFFE URI does not identify the route dataplane",
        ));
    }
    let serial = leaf
        .serial_number()
        .to_bn()
        .and_then(|value| value.to_hex_str())
        .map_err(|error| DomainError::validation(format!("read leaf serial number: {error}")))?;
    let serial_number = canonical_certificate_serial(serial.as_ref())?;
    let not_before = chrono::DateTime::from_timestamp(parsed.validity().not_before.timestamp(), 0)
        .ok_or_else(|| DomainError::validation("leaf notBefore is outside supported range"))?;
    let not_after = chrono::DateTime::from_timestamp(parsed.validity().not_after.timestamp(), 0)
        .ok_or_else(|| DomainError::validation("leaf notAfter is outside supported range"))?;
    Ok(VerifiedExternalCertificate {
        spiffe_uri: spiffe_uri.to_owned(),
        serial_number,
        fingerprint_sha256: format!("{:x}", Sha256::digest(&der)),
        not_before,
        not_after,
    })
}

struct CertificateIssuer {
    ca_certificate_pem: String,
    ca_certificate: X509,
    ca_key: PKey<Private>,
    trust_domain: String,
}

struct IssuedPem {
    certificate_pem: String,
    private_key_pem: String,
    fingerprint_sha256: String,
}

impl CertificateIssuer {
    fn load() -> DomainResult<Self> {
        let ca_cert_path = required_env_path("FLOWPLANE_CERT_ISSUER_CA_CERT_PATH")?;
        let ca_key_path = required_env_path("FLOWPLANE_CERT_ISSUER_CA_KEY_PATH")?;
        let trust_domain = std::env::var("FLOWPLANE_CERT_ISSUER_TRUST_DOMAIN")
            .unwrap_or_else(|_| "flowplane.local".into());
        Self::from_pem(
            read_pem_file(&ca_cert_path, "certificate issuer CA certificate")?,
            read_pem_file(&ca_key_path, "certificate issuer CA key")?,
            trust_domain,
        )
    }

    fn from_pem(
        ca_certificate_pem: String,
        ca_key_pem: String,
        trust_domain: String,
    ) -> DomainResult<Self> {
        validate_trust_domain(&trust_domain)?;
        let ca_certificate = X509::from_pem(ca_certificate_pem.as_bytes()).map_err(|e| {
            DomainError::invalid_config(format!(
                "certificate issuer CA certificate is invalid: {e}"
            ))
        })?;
        let ca_key = PKey::private_key_from_pem(ca_key_pem.as_bytes()).map_err(|e| {
            DomainError::invalid_config(format!("certificate issuer CA key is invalid: {e}"))
        })?;
        validate_issuer_material(&ca_certificate, &ca_key)?;
        Ok(Self {
            ca_certificate_pem,
            ca_certificate,
            ca_key,
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
            .set_issuer_name(self.ca_certificate.subject_name())
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
        let subject_key_identifier = {
            let context = builder.x509v3_context(None, None);
            SubjectKeyIdentifier::new().build(&context).map_err(|e| {
                DomainError::internal(format!("set certificate subject key identifier: {e}"))
            })?
        };
        builder
            .append_extension(subject_key_identifier)
            .map_err(|e| {
                DomainError::internal(format!("append certificate subject key identifier: {e}"))
            })?;
        let authority_key_identifier = {
            let context = builder.x509v3_context(Some(&self.ca_certificate), None);
            AuthorityKeyIdentifier::new()
                .keyid(true)
                .build(&context)
                .map_err(|e| {
                    DomainError::internal(format!("set certificate authority key identifier: {e}"))
                })?
        };
        builder
            .append_extension(authority_key_identifier)
            .map_err(|e| {
                DomainError::internal(format!("append certificate authority key identifier: {e}"))
            })?;
        let san = {
            let context = builder.x509v3_context(Some(&self.ca_certificate), None);
            openssl::x509::extension::SubjectAlternativeName::new()
                .uri(spiffe_uri)
                .build(&context)
                .map_err(|e| DomainError::internal(format!("set certificate SAN: {e}")))?
        };
        builder
            .append_extension(san)
            .map_err(|e| DomainError::internal(format!("append certificate SAN: {e}")))?;
        builder
            .sign(&self.ca_key, MessageDigest::sha256())
            .map_err(|e| DomainError::internal(format!("sign certificate: {e}")))?;
        let cert = builder.build();
        verify_certificate(
            &self.ca_certificate,
            &cert,
            "issued dataplane certificate failed strict SSL-client verification",
            true,
            DomainError::internal,
        )?;
        let certificate_pem = String::from_utf8(
            cert.to_pem()
                .map_err(|e| DomainError::internal(format!("encode certificate PEM: {e}")))?,
        )
        .map_err(|e| DomainError::internal(format!("encode certificate PEM: {e}")))?;
        let fingerprint_sha256 = format!(
            "{:x}",
            Sha256::digest(
                cert.to_der()
                    .map_err(|e| DomainError::internal(format!("encode certificate DER: {e}")))?,
            )
        );
        let private_key_pem = String::from_utf8(
            leaf_key
                .private_key_to_pem_pkcs8()
                .map_err(|e| DomainError::internal(format!("encode private key PEM: {e}")))?,
        )
        .map_err(|e| DomainError::internal(format!("encode private key PEM: {e}")))?;
        Ok(IssuedPem {
            certificate_pem,
            private_key_pem,
            fingerprint_sha256,
        })
    }
}

fn validate_issuer_material(certificate: &X509, private_key: &PKey<Private>) -> DomainResult<()> {
    let public_key = certificate.public_key().map_err(|e| {
        DomainError::invalid_config(format!(
            "certificate issuer CA certificate public key is invalid: {e}"
        ))
    })?;
    if !public_key.public_eq(private_key) {
        return Err(DomainError::invalid_config(
            "certificate issuer CA key does not match the CA certificate public key",
        ));
    }

    let subject_key_id = certificate.subject_key_id().ok_or_else(|| {
        DomainError::invalid_config(
            "certificate issuer CA certificate must contain a Subject Key Identifier",
        )
    })?;
    if subject_key_id.as_slice().is_empty() {
        return Err(DomainError::invalid_config(
            "certificate issuer CA certificate has an empty Subject Key Identifier",
        ));
    }

    let certificate_der = certificate.to_der().map_err(|e| {
        DomainError::invalid_config(format!(
            "certificate issuer CA certificate cannot be decoded: {e}"
        ))
    })?;
    let (_, parsed_certificate) =
        x509_parser::parse_x509_certificate(&certificate_der).map_err(|e| {
            DomainError::invalid_config(format!(
                "certificate issuer CA certificate cannot be decoded: {e}"
            ))
        })?;
    let mut is_ca = false;
    let mut can_sign_certificates = false;
    let mut allows_client_auth = true;
    for extension in parsed_certificate.extensions() {
        match extension.parsed_extension() {
            x509_parser::extensions::ParsedExtension::BasicConstraints(constraints) => {
                is_ca = constraints.ca;
            }
            x509_parser::extensions::ParsedExtension::KeyUsage(usage) => {
                can_sign_certificates = usage.key_cert_sign();
            }
            x509_parser::extensions::ParsedExtension::ExtendedKeyUsage(usage) => {
                allows_client_auth = usage.any || usage.client_auth;
            }
            _ => {}
        }
    }
    if !is_ca {
        return Err(DomainError::invalid_config(
            "certificate issuer CA certificate must contain Basic Constraints with CA:TRUE",
        ));
    }
    if !can_sign_certificates {
        return Err(DomainError::invalid_config(
            "certificate issuer CA certificate Key Usage must include keyCertSign",
        ));
    }
    if !allows_client_auth {
        return Err(DomainError::invalid_config(
            "certificate issuer CA certificate Extended Key Usage must allow clientAuth",
        ));
    }

    let now = Asn1Time::from_unix(chrono::Utc::now().timestamp()).map_err(|e| {
        DomainError::internal(format!("prepare certificate issuer validation time: {e}"))
    })?;
    if certificate
        .not_before()
        .compare(&now)
        .map_err(|e| DomainError::invalid_config(format!("read CA notBefore: {e}")))?
        .is_gt()
    {
        return Err(DomainError::invalid_config(
            "certificate issuer CA certificate is not yet valid",
        ));
    }
    if certificate
        .not_after()
        .compare(&now)
        .map_err(|e| DomainError::invalid_config(format!("read CA notAfter: {e}")))?
        .is_lt()
    {
        return Err(DomainError::invalid_config(
            "certificate issuer CA certificate has expired",
        ));
    }

    verify_certificate(
        certificate,
        certificate,
        "certificate issuer CA certificate is unsuitable",
        false,
        DomainError::invalid_config,
    )
}

fn verify_certificate(
    trust_anchor: &X509,
    certificate: &X509,
    context: &str,
    require_ssl_client_purpose: bool,
    error: fn(String) -> DomainError,
) -> DomainResult<()> {
    let mut store = X509StoreBuilder::new()
        .map_err(|e| error(format!("{context}: create trust store: {e}")))?;
    store
        .set_flags(X509VerifyFlags::X509_STRICT | X509VerifyFlags::PARTIAL_CHAIN)
        .map_err(|e| {
            error(format!(
                "{context}: configure strict partial-chain policy: {e}"
            ))
        })?;
    if require_ssl_client_purpose {
        store
            .set_purpose(X509PurposeId::SSL_CLIENT)
            .map_err(|e| error(format!("{context}: configure SSL-client purpose: {e}")))?;
    }
    store
        .add_cert(trust_anchor.to_owned())
        .map_err(|e| error(format!("{context}: add trust anchor: {e}")))?;
    let store = store.build();
    let chain = Stack::new().map_err(|e| error(format!("{context}: create chain: {e}")))?;
    let mut store_context = X509StoreContext::new()
        .map_err(|e| error(format!("{context}: create verification context: {e}")))?;
    let (verified, verify_error) = store_context
        .init(&store, certificate, &chain, |ctx| {
            let verified = ctx.verify_cert()?;
            Ok((verified, ctx.error()))
        })
        .map_err(|e| error(format!("{context}: verification failed to execute: {e}")))?;
    if !verified {
        return Err(error(format!("{context}: {}", verify_error.error_string())));
    }
    Ok(())
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::{required_env_path, CertificateIssuer};
    use fp_domain::error::ErrorCode;
    use openssl::asn1::Asn1Time;
    use openssl::bn::BigNum;
    use openssl::hash::MessageDigest;
    use openssl::pkey::{PKey, Private};
    use openssl::rsa::Rsa;
    use openssl::x509::extension::{
        AuthorityKeyIdentifier, BasicConstraints, KeyUsage, SubjectKeyIdentifier,
    };
    use openssl::x509::{X509NameBuilder, X509};

    #[derive(Clone, Copy)]
    struct CaProfile {
        basic_constraints: bool,
        key_cert_sign: bool,
        subject_key_id: bool,
        server_auth_only: bool,
        not_before: i64,
        not_after: i64,
    }

    impl CaProfile {
        fn valid() -> Self {
            let now = chrono::Utc::now().timestamp();
            Self {
                basic_constraints: true,
                key_cert_sign: true,
                subject_key_id: true,
                server_auth_only: false,
                not_before: now - 60,
                not_after: now + 86_400,
            }
        }
    }

    fn key() -> PKey<Private> {
        PKey::from_rsa(Rsa::generate(2048).expect("RSA key")).expect("private key")
    }

    fn build_ca(
        common_name: &str,
        public_key: &PKey<Private>,
        signing_key: &PKey<Private>,
        issuer: Option<&X509>,
        profile: CaProfile,
    ) -> X509 {
        let mut builder = X509::builder().expect("X509 builder");
        builder.set_version(2).expect("version");
        let serial = BigNum::from_u32(1)
            .and_then(|number| number.to_asn1_integer())
            .expect("serial");
        builder.set_serial_number(&serial).expect("serial");
        let mut name = X509NameBuilder::new().expect("name");
        name.append_entry_by_text("CN", common_name).expect("CN");
        let name = name.build();
        builder.set_subject_name(&name).expect("subject");
        match issuer {
            Some(issuer) => builder
                .set_issuer_name(issuer.subject_name())
                .expect("issuer"),
            None => builder.set_issuer_name(&name).expect("issuer"),
        }
        builder.set_pubkey(public_key).expect("public key");
        let not_before = Asn1Time::from_unix(profile.not_before).expect("notBefore");
        let not_after = Asn1Time::from_unix(profile.not_after).expect("notAfter");
        builder.set_not_before(&not_before).expect("notBefore");
        builder.set_not_after(&not_after).expect("notAfter");
        if profile.basic_constraints {
            builder
                .append_extension(
                    BasicConstraints::new()
                        .critical()
                        .ca()
                        .build()
                        .expect("basic constraints"),
                )
                .expect("basic constraints");
        }
        let key_usage = if profile.key_cert_sign {
            KeyUsage::new()
                .critical()
                .key_cert_sign()
                .crl_sign()
                .build()
                .expect("CA key usage")
        } else {
            KeyUsage::new()
                .critical()
                .digital_signature()
                .build()
                .expect("non-CA key usage")
        };
        builder
            .append_extension(key_usage)
            .expect("key usage extension");
        if profile.subject_key_id {
            let subject_key_id = {
                let context = builder.x509v3_context(None, None);
                SubjectKeyIdentifier::new().build(&context).expect("SKI")
            };
            builder.append_extension(subject_key_id).expect("SKI");
        }
        if profile.server_auth_only {
            builder
                .append_extension(
                    openssl::x509::extension::ExtendedKeyUsage::new()
                        .server_auth()
                        .build()
                        .expect("server-only EKU"),
                )
                .expect("server-only EKU");
        }
        if let Some(issuer) = issuer {
            let authority_key_id = {
                let context = builder.x509v3_context(Some(issuer), None);
                AuthorityKeyIdentifier::new()
                    .keyid(true)
                    .build(&context)
                    .expect("AKI")
            };
            builder.append_extension(authority_key_id).expect("AKI");
        }
        builder
            .sign(signing_key, MessageDigest::sha256())
            .expect("sign CA");
        builder.build()
    }

    fn root_material(profile: CaProfile) -> (String, String) {
        let root_key = key();
        let template = build_ca(
            "Flowplane Root",
            &root_key,
            &root_key,
            None,
            CaProfile::valid(),
        );
        let root = build_ca(
            "Flowplane Root",
            &root_key,
            &root_key,
            Some(&template),
            profile,
        );
        (
            String::from_utf8(root.to_pem().expect("certificate PEM")).expect("certificate UTF-8"),
            String::from_utf8(
                root_key
                    .private_key_to_pem_pkcs8()
                    .expect("private-key PEM"),
            )
            .expect("private-key UTF-8"),
        )
    }

    fn issuer_from(profile: CaProfile) -> Result<CertificateIssuer, fp_domain::DomainError> {
        let (certificate, private_key) = root_material(profile);
        CertificateIssuer::from_pem(certificate, private_key, "flowplane.test".into())
    }

    fn issuer_error(
        result: Result<CertificateIssuer, fp_domain::DomainError>,
        expectation: &str,
    ) -> fp_domain::DomainError {
        match result {
            Ok(_) => panic!("{expectation}"),
            Err(error) => error,
        }
    }

    #[test]
    fn standards_complete_root_issuer_is_accepted() {
        issuer_from(CaProfile::valid()).expect("valid root issuer");
    }

    #[test]
    fn issued_leaf_exposes_sha256_der_fingerprint() {
        use sha2::{Digest, Sha256};

        let issuer = issuer_from(CaProfile::valid()).expect("valid root issuer");
        let issued = issuer
            .issue(
                "dp-test",
                "spiffe://flowplane.test/org/o/team/t/proxy/p",
                "a",
                chrono::Utc::now() + chrono::Duration::hours(1),
            )
            .expect("issue leaf");
        let certificate = X509::from_pem(issued.certificate_pem.as_bytes()).expect("leaf PEM");
        let expected = format!(
            "{:x}",
            Sha256::digest(certificate.to_der().expect("leaf DER"))
        );
        assert_eq!(issued.fingerprint_sha256, expected);
    }

    #[test]
    fn standards_complete_intermediate_issuer_is_accepted_as_partial_chain_anchor() {
        let root_key = key();
        let root_template = build_ca(
            "Flowplane Root",
            &root_key,
            &root_key,
            None,
            CaProfile::valid(),
        );
        let root = build_ca(
            "Flowplane Root",
            &root_key,
            &root_key,
            Some(&root_template),
            CaProfile::valid(),
        );
        let intermediate_key = key();
        let intermediate = build_ca(
            "Flowplane Intermediate",
            &intermediate_key,
            &root_key,
            Some(&root),
            CaProfile::valid(),
        );
        CertificateIssuer::from_pem(
            String::from_utf8(intermediate.to_pem().expect("certificate PEM"))
                .expect("certificate UTF-8"),
            String::from_utf8(
                intermediate_key
                    .private_key_to_pem_pkcs8()
                    .expect("private-key PEM"),
            )
            .expect("private-key UTF-8"),
            "flowplane.test".into(),
        )
        .expect("valid intermediate issuer");
    }

    #[test]
    fn mismatched_issuer_private_key_is_rejected_actionably() {
        let (certificate, _) = root_material(CaProfile::valid());
        let wrong_key = String::from_utf8(
            key()
                .private_key_to_pem_pkcs8()
                .expect("wrong private-key PEM"),
        )
        .expect("private-key UTF-8");
        let error = issuer_error(
            CertificateIssuer::from_pem(certificate, wrong_key, "flowplane.test".into()),
            "mismatched key must fail",
        );
        assert_eq!(error.code, ErrorCode::InvalidConfig);
        assert!(
            error.message.contains("does not match"),
            "{}",
            error.message
        );
    }

    #[test]
    fn malformed_issuer_certificate_and_private_key_are_rejected_actionably() {
        let (_, valid_key) = root_material(CaProfile::valid());
        let bad_certificate = issuer_error(
            CertificateIssuer::from_pem(
                "not a certificate".into(),
                valid_key,
                "flowplane.test".into(),
            ),
            "malformed certificate must fail",
        );
        assert!(
            bad_certificate
                .message
                .contains("CA certificate is invalid"),
            "{}",
            bad_certificate.message
        );

        let (valid_certificate, _) = root_material(CaProfile::valid());
        let bad_key = issuer_error(
            CertificateIssuer::from_pem(
                valid_certificate,
                "not a private key".into(),
                "flowplane.test".into(),
            ),
            "malformed private key must fail",
        );
        assert!(
            bad_key.message.contains("CA key is invalid"),
            "{}",
            bad_key.message
        );
    }

    #[test]
    fn issuer_without_subject_key_identifier_is_rejected_actionably() {
        let error = issuer_error(
            issuer_from(CaProfile {
                subject_key_id: false,
                ..CaProfile::valid()
            }),
            "missing SKI must fail",
        );
        assert_eq!(error.code, ErrorCode::InvalidConfig);
        assert!(
            error.message.contains("Subject Key Identifier"),
            "{}",
            error.message
        );
    }

    #[test]
    fn expired_and_not_yet_valid_issuers_are_rejected_actionably() {
        let now = chrono::Utc::now().timestamp();
        let expired = issuer_error(
            issuer_from(CaProfile {
                not_before: now - 172_800,
                not_after: now - 86_400,
                ..CaProfile::valid()
            }),
            "expired issuer must fail",
        );
        assert!(expired.message.contains("expired"), "{}", expired.message);

        let future = issuer_error(
            issuer_from(CaProfile {
                not_before: now + 86_400,
                not_after: now + 172_800,
                ..CaProfile::valid()
            }),
            "future issuer must fail",
        );
        assert!(
            future.message.contains("not yet valid"),
            "{}",
            future.message
        );
    }

    #[test]
    fn issuer_without_ca_constraints_or_cert_sign_usage_is_rejected_actionably() {
        let no_constraints = issuer_error(
            issuer_from(CaProfile {
                basic_constraints: false,
                ..CaProfile::valid()
            }),
            "missing CA constraints must fail",
        );
        assert!(
            no_constraints.message.contains("Basic Constraints"),
            "{}",
            no_constraints.message
        );

        let no_cert_sign = issuer_error(
            issuer_from(CaProfile {
                key_cert_sign: false,
                ..CaProfile::valid()
            }),
            "missing keyCertSign must fail",
        );
        assert!(
            no_cert_sign.message.contains("keyCertSign"),
            "{}",
            no_cert_sign.message
        );
    }

    #[test]
    fn issuer_with_restrictive_extended_key_usage_is_rejected_actionably() {
        let error = issuer_error(
            issuer_from(CaProfile {
                server_auth_only: true,
                ..CaProfile::valid()
            }),
            "server-only issuer EKU must fail",
        );
        assert_eq!(error.code, ErrorCode::InvalidConfig);
        assert!(error.message.contains("clientAuth"), "{}", error.message);
    }

    // Obs-1 (fpv2-86m.4): an unconfigured cert-issuer prerequisite must fail closed with a
    // *reason* — the error names the missing env var and carries an actionable hint, so the
    // operator log (fp-api logs message + hint) is actionable even though the client sees the
    // redacted generic 500. Reads a guaranteed-unset var, so it is parallel-safe (no env mutation).
    #[test]
    fn unconfigured_cert_issuer_env_fails_with_named_reason() {
        let err = required_env_path("FLOWPLANE_CERT_ISSUER_CA_CERT_PATH_DEFINITELY_UNSET_XQ7")
            .expect_err("an unset env path must error");
        assert_eq!(err.code, ErrorCode::InvalidConfig);
        assert!(
            err.message
                .contains("FLOWPLANE_CERT_ISSUER_CA_CERT_PATH_DEFINITELY_UNSET_XQ7"),
            "error message must name the missing env var: {}",
            err.message
        );
        let hint = err.hint.unwrap_or_default();
        assert!(
            hint.contains("FLOWPLANE_CERT_ISSUER_CA_CERT_PATH")
                && hint.contains("FLOWPLANE_CERT_ISSUER_CA_KEY_PATH"),
            "hint must name the cert-issuer prerequisites: {hint}"
        );
    }
}
