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

const DEFAULT_KEY_ID: &str = "default";
const ACTIVE_KEY_ID_ENV: &str = "FLOWPLANE_SECRET_ENCRYPTION_KEY_ID";
const ACTIVE_KEY_ENV: &str = "FLOWPLANE_SECRET_ENCRYPTION_KEY";

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

#[derive(Debug, Clone)]
pub struct SecretRotate<'a> {
    pub name: &'a str,
    pub expected_version: i64,
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
    crate::services::quota::check_team_resource_quota(pool, team.id, Resource::Secrets).await?;
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
        &encrypted.key_id,
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
    rotate: SecretRotate<'_>,
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
    rotate.spec.validate()?;
    validate_expiry(rotate.expires_at)?;
    let encrypted = encrypt_spec(&rotate.spec)?;

    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("rotate secret: begin"))?;
    let secret = secrets::rotate_secret(
        &mut tx,
        team.id,
        rotate.name,
        rotate.expected_version,
        rotate.spec.secret_type(),
        &encrypted.ciphertext,
        &encrypted.nonce,
        &encrypted.key_id,
        rotate.expires_at,
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
            &format!("secrets/{}", rotate.name),
        ),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("rotate secret: commit"))?;
    Ok(secret)
}

struct EncryptedSpec {
    key_id: String,
    ciphertext: Vec<u8>,
    nonce: [u8; 12],
}

fn encrypt_spec(spec: &SecretSpec) -> DomainResult<EncryptedSpec> {
    let key = active_secret_key()?;
    let mut nonce = [0_u8; 12];
    getrandom::fill(&mut nonce)
        .map_err(|e| DomainError::internal(format!("generate secret nonce: {e}")))?;
    let plaintext = serde_json::to_vec(spec)
        .map_err(|e| DomainError::internal(format!("serialize secret spec: {e}")))?;
    let cipher = Aes256Gcm::new_from_slice(&key.bytes)
        .map_err(|_| DomainError::invalid_config("FLOWPLANE_SECRET_ENCRYPTION_KEY is invalid"))?;
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce), plaintext.as_ref())
        .map_err(|_| DomainError::internal("encrypt secret spec"))?;
    Ok(EncryptedSpec {
        key_id: key.id,
        ciphertext,
        nonce,
    })
}

struct SecretKey {
    id: String,
    bytes: [u8; 32],
}

fn active_secret_key() -> DomainResult<SecretKey> {
    let id = active_key_id()?;
    let raw = std::env::var(ACTIVE_KEY_ENV).map_err(|_| {
        DomainError::unavailable("secret encryption key is not configured")
            .with_hint("set FLOWPLANE_SECRET_ENCRYPTION_KEY to a 32-byte or base64-encoded key")
    })?;
    Ok(SecretKey {
        id,
        bytes: parse_secret_key(ACTIVE_KEY_ENV, &raw)?,
    })
}

fn active_key_id() -> DomainResult<String> {
    let id = std::env::var(ACTIVE_KEY_ID_ENV).unwrap_or_else(|_| DEFAULT_KEY_ID.to_string());
    validate_key_id(&id)?;
    Ok(id)
}

fn validate_key_id(id: &str) -> DomainResult<()> {
    if id.is_empty() || id.len() > 128 || id.chars().any(|c| c.is_control() || c == '\0') {
        return Err(DomainError::invalid_config(format!(
            "{ACTIVE_KEY_ID_ENV} must be 1..=128 printable characters"
        )));
    }
    Ok(())
}

fn parse_secret_key(label: &str, raw: &str) -> DomainResult<[u8; 32]> {
    if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(raw) {
        if let Ok(key) = <[u8; 32]>::try_from(bytes.as_slice()) {
            return Ok(key);
        }
    }
    if let Ok(bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(raw) {
        if let Ok(key) = <[u8; 32]>::try_from(bytes.as_slice()) {
            return Ok(key);
        }
    }
    <[u8; 32]>::try_from(raw.as_bytes()).map_err(|_| {
        DomainError::invalid_config(format!("{label} must be exactly 32 bytes after decoding"))
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
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn raw_32_byte_key_is_accepted() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        std::env::set_var(
            "FLOWPLANE_SECRET_ENCRYPTION_KEY",
            "12345678901234567890123456789012",
        );
        std::env::remove_var("FLOWPLANE_SECRET_ENCRYPTION_KEY_ID");
        assert!(active_secret_key().is_ok());
    }

    #[test]
    fn active_key_id_is_written_with_ciphertext() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        std::env::set_var(
            "FLOWPLANE_SECRET_ENCRYPTION_KEY",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        );
        std::env::set_var("FLOWPLANE_SECRET_ENCRYPTION_KEY_ID", "v2");
        let encrypted = encrypt_spec(&SecretSpec::GenericSecret {
            secret: "c2VjcmV0".into(),
        })
        .expect("encrypt");
        assert_eq!(encrypted.key_id, "v2");
    }
}
