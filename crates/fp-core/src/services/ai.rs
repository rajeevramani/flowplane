//! AI gateway services.

use crate::authz::{check_resource_access, Decision, PrincipalCtx};
use crate::services::{actor_of, deny_to_error, record_authz_denial};
use fp_domain::authz::{Action, Resource, TeamRef};
use fp_domain::{
    validate_ai_provider_name, AiProvider, AiProviderSpec, DomainError, DomainResult, RequestId,
};
use fp_storage::repos::{ai, audit};
use sqlx::PgPool;

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

pub async fn create_provider(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    spec: AiProviderSpec,
    request_id: RequestId,
) -> DomainResult<AiProvider> {
    authorize(
        pool,
        ctx,
        Resource::AiProviders,
        Action::Create,
        team,
        request_id,
    )
    .await?;
    validate_ai_provider_name(name)?;
    validate_provider_spec(pool, ctx, team, &spec, request_id).await?;
    crate::services::quota::check_team_resource_quota(pool, team.id, Resource::AiProviders).await?;

    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("create AI provider: begin"))?;
    let provider = ai::create(&mut tx, team, name, &spec).await?;
    audit::record_in_tx(
        &mut tx,
        &mutation_audit(ctx, request_id, team, "ai_provider.create", name),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("create AI provider: commit"))?;
    Ok(provider)
}

pub async fn list_providers(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    limit: i64,
    offset: i64,
    request_id: RequestId,
) -> DomainResult<(Vec<AiProvider>, i64)> {
    authorize(
        pool,
        ctx,
        Resource::AiProviders,
        Action::Read,
        team,
        request_id,
    )
    .await?;
    ai::list(pool, team.id, limit, offset).await
}

pub async fn get_provider(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    request_id: RequestId,
) -> DomainResult<AiProvider> {
    authorize(
        pool,
        ctx,
        Resource::AiProviders,
        Action::Read,
        team,
        request_id,
    )
    .await?;
    ai::get(pool, team.id, name)
        .await?
        .ok_or_else(|| DomainError::not_found("AI provider", name))
}

pub async fn update_provider(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    spec: AiProviderSpec,
    expected_version: i64,
    request_id: RequestId,
) -> DomainResult<AiProvider> {
    authorize(
        pool,
        ctx,
        Resource::AiProviders,
        Action::Update,
        team,
        request_id,
    )
    .await?;
    validate_provider_spec(pool, ctx, team, &spec, request_id).await?;
    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("update AI provider: begin"))?;
    let provider = ai::update(&mut tx, team.id, name, &spec, expected_version).await?;
    audit::record_in_tx(
        &mut tx,
        &mutation_audit(ctx, request_id, team, "ai_provider.update", name),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("update AI provider: commit"))?;
    Ok(provider)
}

pub async fn delete_provider(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    expected_version: i64,
    request_id: RequestId,
) -> DomainResult<()> {
    authorize(
        pool,
        ctx,
        Resource::AiProviders,
        Action::Delete,
        team,
        request_id,
    )
    .await?;
    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("delete AI provider: begin"))?;
    ai::delete(&mut tx, team.id, name, expected_version).await?;
    audit::record_in_tx(
        &mut tx,
        &mutation_audit(ctx, request_id, team, "ai_provider.delete", name),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("delete AI provider: commit"))?;
    Ok(())
}

async fn validate_provider_spec(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    spec: &AiProviderSpec,
    request_id: RequestId,
) -> DomainResult<()> {
    spec.validate()?;
    authorize(pool, ctx, Resource::Secrets, Action::Read, team, request_id).await?;
    let secret =
        fp_storage::repos::secrets::get_secret_by_id(pool, team.id, spec.credential_secret_id)
            .await?
            .ok_or_else(|| {
                let id = spec.credential_secret_id.to_string();
                DomainError::not_found("secret", &id)
            })?;
    if !matches!(secret.secret_type, fp_domain::SecretType::GenericSecret) {
        return Err(DomainError::validation(
            "AI provider credential_secret_id must reference a generic_secret",
        ));
    }
    Ok(())
}

fn mutation_audit(
    ctx: &PrincipalCtx,
    request_id: RequestId,
    team: TeamRef,
    action: &str,
    name: &str,
) -> audit::AuditEntry {
    let (actor_type, actor_id) = actor_of(ctx);
    audit::AuditEntry {
        request_id: Some(request_id),
        actor_type,
        actor_id,
        actor_label: String::new(),
        surface: audit::Surface::Rest,
        action: action.into(),
        resource: format!("ai-providers/{name}"),
        org_id: Some(team.org_id),
        team_id: Some(team.id),
        outcome: audit::Outcome::Success,
        detail: serde_json::json!({}),
    }
}
