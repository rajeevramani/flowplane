//! Dataplane + proxy-certificate services (S5.4). REST exposure lands with S6; the xDS
//! mTLS path and tests drive these directly. Same contract as every service: one
//! transaction holding the row change, its outbox event, and its audit entry.

use crate::authz::{check_resource_access, Decision, PrincipalCtx};
use crate::services::{actor_of, deny_to_error, trace_context_json};
use fp_domain::authz::{Action, Resource, TeamRef};
use fp_domain::dataplane::{validate_spiffe_uri, Dataplane, ProxyCertificate};
use fp_domain::event::{DomainEvent, EventScope};
use fp_domain::{validate_name, DomainResult, RequestId, UserId};
use fp_storage::repos::{audit, dataplanes};
use sqlx::PgPool;

fn authorize(
    ctx: &PrincipalCtx,
    resource: Resource,
    action: Action,
    team: TeamRef,
) -> DomainResult<()> {
    match check_resource_access(ctx, resource, action, Some(team)) {
        Decision::Allow(_) => Ok(()),
        Decision::Deny(reason) => Err(deny_to_error(resource, action, reason)),
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
    authorize(ctx, Resource::Dataplanes, Action::Create, team)?;
    validate_name(name)?;
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
) -> DomainResult<Dataplane> {
    authorize(ctx, Resource::Dataplanes, Action::Read, team)?;
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
) -> DomainResult<(Vec<Dataplane>, i64)> {
    authorize(ctx, Resource::Dataplanes, Action::Read, team)?;
    dataplanes::list_dataplanes(pool, team.id, limit, offset).await
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

/// Register an issued certificate against a dataplane. The full SPIFFE URI becomes the
/// mTLS binding key; the cert's own team/proxy segments are never trusted at runtime.
pub async fn register_certificate(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    registration: CertificateRegistration<'_>,
    request_id: RequestId,
) -> DomainResult<ProxyCertificate> {
    authorize(ctx, Resource::ProxyCertificates, Action::Create, team)?;
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
    authorize(ctx, Resource::ProxyCertificates, Action::Update, team)?;
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
