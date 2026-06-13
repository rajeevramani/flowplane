//! Secret services (S6.3). Secret values enter only on create/rotate, are encrypted before
//! storage, and read paths return metadata only.

use crate::authz::{check_resource_access, Decision, PrincipalCtx};
use crate::services::{actor_of, deny_to_error, record_authz_denial, trace_context_json};
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine as _;
use fp_domain::authz::{Action, Resource, TeamRef};
use fp_domain::event::{DomainEvent, EventScope};
use fp_domain::{validate_name, DomainError, DomainResult, RequestId, Secret, SecretSpec};
use fp_storage::repos::{audit, secrets};
use sqlx::PgPool;

const KEY_ID: &str = "default";

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

#[derive(Debug, Clone)]
pub struct SecretWrite<'a> {
    pub name: &'a str,
    pub description: &'a str,
    pub spec: SecretSpec,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub async fn create_secret(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    write: SecretWrite<'_>,
    request_id: RequestId,
) -> DomainResult<Secret> {
    authorize(
        pool,
        ctx,
        Resource::Secrets,
        Action::Create,
        team,
        request_id,
    )
    .await?;
    validate_name(write.name)?;
    write.spec.validate()?;
    validate_expiry(write.expires_at)?;
    let encrypted = encrypt_spec(&write.spec)?;

    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("create secret: begin"))?;
    let secret = secrets::create_secret(
        &mut tx,
        team,
        write.name,
        write.description,
        write.spec.secret_type(),
        &encrypted.ciphertext,
        &encrypted.nonce,
        KEY_ID,
        write.expires_at,
    )
    .await?;
    fp_storage::outbox::append(
        &mut tx,
        &DomainEvent::SecretUpserted {
            secret_id: secret.id.as_uuid(),
            name: secret.name.clone(),
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
            "secret.create",
            &format!("secrets/{}", write.name),
        ),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("create secret: commit"))?;
    Ok(secret)
}

pub async fn list_secrets(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    limit: i64,
    offset: i64,
    request_id: RequestId,
) -> DomainResult<(Vec<Secret>, i64)> {
    authorize(pool, ctx, Resource::Secrets, Action::Read, team, request_id).await?;
    secrets::list_secrets(pool, team.id, limit, offset).await
}

pub async fn get_secret(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    request_id: RequestId,
) -> DomainResult<Secret> {
    authorize(pool, ctx, Resource::Secrets, Action::Read, team, request_id).await?;
    secrets::get_secret(pool, team.id, name)
        .await?
        .ok_or_else(|| DomainError::not_found("secret", name))
}

pub async fn rotate_secret(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    spec: SecretSpec,
    expires_at: Option<chrono::DateTime<chrono::Utc>>,
    request_id: RequestId,
) -> DomainResult<Secret> {
    authorize(
        pool,
        ctx,
        Resource::Secrets,
        Action::Update,
        team,
        request_id,
    )
    .await?;
    spec.validate()?;
    validate_expiry(expires_at)?;
    let encrypted = encrypt_spec(&spec)?;

    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("rotate secret: begin"))?;
    let secret = secrets::rotate_secret(
        &mut tx,
        team.id,
        name,
        spec.secret_type(),
        &encrypted.ciphertext,
        &encrypted.nonce,
        KEY_ID,
        expires_at,
    )
    .await?;
    fp_storage::outbox::append(
        &mut tx,
        &DomainEvent::SecretUpserted {
            secret_id: secret.id.as_uuid(),
            name: secret.name.clone(),
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
            "secret.rotate",
            &format!("secrets/{name}"),
        ),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("rotate secret: commit"))?;
    Ok(secret)
}

struct EncryptedSpec {
    ciphertext: Vec<u8>,
    nonce: [u8; 12],
}

fn encrypt_spec(spec: &SecretSpec) -> DomainResult<EncryptedSpec> {
    let key = secret_key()?;
    let mut nonce = [0_u8; 12];
    getrandom::fill(&mut nonce)
        .map_err(|e| DomainError::internal(format!("generate secret nonce: {e}")))?;
    let plaintext = serde_json::to_vec(spec)
        .map_err(|e| DomainError::internal(format!("serialize secret spec: {e}")))?;
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|_| DomainError::invalid_config("FLOWPLANE_SECRET_ENCRYPTION_KEY is invalid"))?;
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce), plaintext.as_ref())
        .map_err(|_| DomainError::internal("encrypt secret spec"))?;
    Ok(EncryptedSpec { ciphertext, nonce })
}

fn secret_key() -> DomainResult<[u8; 32]> {
    let raw = std::env::var("FLOWPLANE_SECRET_ENCRYPTION_KEY").map_err(|_| {
        DomainError::unavailable("secret encryption key is not configured")
            .with_hint("set FLOWPLANE_SECRET_ENCRYPTION_KEY to a 32-byte or base64-encoded key")
    })?;
    if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(&raw) {
        if let Ok(key) = <[u8; 32]>::try_from(bytes.as_slice()) {
            return Ok(key);
        }
    }
    if let Ok(bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(&raw) {
        if let Ok(key) = <[u8; 32]>::try_from(bytes.as_slice()) {
            return Ok(key);
        }
    }
    <[u8; 32]>::try_from(raw.as_bytes()).map_err(|_| {
        DomainError::invalid_config(
            "FLOWPLANE_SECRET_ENCRYPTION_KEY must be exactly 32 bytes after decoding",
        )
    })
}

fn validate_expiry(expires_at: Option<chrono::DateTime<chrono::Utc>>) -> DomainResult<()> {
    if expires_at.is_some_and(|ts| ts <= chrono::Utc::now()) {
        return Err(DomainError::validation(
            "secret expiry must be in the future",
        ));
    }
    Ok(())
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

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn raw_32_byte_key_is_accepted() {
        std::env::set_var(
            "FLOWPLANE_SECRET_ENCRYPTION_KEY",
            "12345678901234567890123456789012",
        );
        assert!(secret_key().is_ok());
    }
}
