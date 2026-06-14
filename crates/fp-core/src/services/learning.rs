//! Learning session services (S8.3): durable capture-session lifecycle before injection.

use crate::authz::{check_resource_access, Decision, PrincipalCtx};
use crate::services::{actor_of, deny_to_error, record_authz_denial, trace_context_json};
use fp_domain::api_lifecycle::{CaptureSession, CaptureSessionSpec, CaptureSessionStatus};
use fp_domain::authz::{Action, Resource, TeamRef};
use fp_domain::event::{DomainEvent, EventScope};
use fp_domain::{ApiDefinitionId, DomainError, DomainResult, RequestId};
use fp_storage::repos::{api_lifecycle, audit};
use sqlx::PgPool;

#[derive(Debug, Clone)]
pub struct StartLearningSessionInput {
    pub name: String,
    pub api: Option<String>,
    pub spec: CaptureSessionSpec,
}

async fn authorize(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    action: Action,
    team: TeamRef,
    request_id: RequestId,
) -> DomainResult<()> {
    match check_resource_access(ctx, Resource::LearningSessions, action, Some(team)) {
        Decision::Allow(_) => Ok(()),
        Decision::Deny(reason) => {
            record_authz_denial(
                pool,
                ctx,
                request_id,
                Resource::LearningSessions,
                action,
                Some(team),
                reason,
            )
            .await;
            Err(deny_to_error(Resource::LearningSessions, action, reason))
        }
    }
}

fn mutation_audit(
    ctx: &PrincipalCtx,
    request_id: RequestId,
    team: TeamRef,
    action: &str,
    resource: String,
) -> audit::AuditEntry {
    let (actor_type, actor_id) = actor_of(ctx);
    audit::AuditEntry {
        request_id: Some(request_id),
        actor_type,
        actor_id,
        actor_label: String::new(),
        surface: audit::Surface::Rest,
        action: action.into(),
        resource,
        org_id: Some(team.org_id),
        team_id: Some(team.id),
        outcome: audit::Outcome::Success,
        detail: serde_json::json!({}),
    }
}

pub async fn start_session(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    input: StartLearningSessionInput,
    request_id: RequestId,
) -> DomainResult<CaptureSession> {
    authorize(pool, ctx, Action::Create, team, request_id).await?;
    let spec = resolve_api_name(pool, team, input.api, input.spec).await?;
    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("start learning session: begin"))?;
    let session = api_lifecycle::create_capture_session(&mut tx, team, &input.name, &spec).await?;
    fp_storage::outbox::append(
        &mut tx,
        &DomainEvent::CaptureSessionStarted {
            capture_session_id: session.id.as_uuid(),
            name: session.name.clone(),
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
            "learn.start",
            format!("learning-sessions/{}", session.name),
        ),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("start learning session: commit"))?;
    Ok(session)
}

pub async fn list_sessions(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    status: Option<CaptureSessionStatus>,
    limit: i64,
    offset: i64,
    request_id: RequestId,
) -> DomainResult<(Vec<CaptureSession>, i64)> {
    authorize(pool, ctx, Action::Read, team, request_id).await?;
    api_lifecycle::list_capture_sessions(pool, team.id, status, limit, offset).await
}

pub async fn get_session(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    session: &str,
    request_id: RequestId,
) -> DomainResult<CaptureSession> {
    authorize(pool, ctx, Action::Read, team, request_id).await?;
    api_lifecycle::get_capture_session(pool, team.id, session)
        .await?
        .ok_or_else(|| DomainError::not_found("learning session", session))
}

pub async fn stop_session(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    session: &str,
    request_id: RequestId,
) -> DomainResult<CaptureSession> {
    transition_session(
        pool,
        ctx,
        team,
        session,
        SessionTransition {
            status: CaptureSessionStatus::Completed,
            action: Action::Execute,
            audit_action: "learn.stop",
        },
        request_id,
    )
    .await
}

pub async fn cancel_session(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    session: &str,
    request_id: RequestId,
) -> DomainResult<CaptureSession> {
    transition_session(
        pool,
        ctx,
        team,
        session,
        SessionTransition {
            status: CaptureSessionStatus::Cancelled,
            action: Action::Delete,
            audit_action: "learn.cancel",
        },
        request_id,
    )
    .await
}

#[derive(Debug, Clone, Copy)]
struct SessionTransition {
    status: CaptureSessionStatus,
    action: Action,
    audit_action: &'static str,
}

async fn transition_session(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    session: &str,
    transition: SessionTransition,
    request_id: RequestId,
) -> DomainResult<CaptureSession> {
    authorize(pool, ctx, transition.action, team, request_id).await?;
    let mut tx = pool.begin().await.map_err(crate::services::db_err(
        "transition learning session: begin",
    ))?;
    let updated =
        api_lifecycle::transition_capture_session(&mut tx, team.id, session, transition.status)
            .await?;
    let event = match transition.status {
        CaptureSessionStatus::Completed => DomainEvent::CaptureSessionStopped {
            capture_session_id: updated.id.as_uuid(),
            name: updated.name.clone(),
        },
        CaptureSessionStatus::Cancelled => DomainEvent::CaptureSessionCancelled {
            capture_session_id: updated.id.as_uuid(),
            name: updated.name.clone(),
        },
        CaptureSessionStatus::Capturing | CaptureSessionStatus::Failed => {
            return Err(DomainError::internal(
                "unsupported learning session service transition",
            ));
        }
    };
    fp_storage::outbox::append(
        &mut tx,
        &event,
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
            transition.audit_action,
            format!("learning-sessions/{}", updated.name),
        ),
    )
    .await?;
    tx.commit().await.map_err(crate::services::db_err(
        "transition learning session: commit",
    ))?;
    Ok(updated)
}

async fn resolve_api_name(
    pool: &PgPool,
    team: TeamRef,
    api: Option<String>,
    mut spec: CaptureSessionSpec,
) -> DomainResult<CaptureSessionSpec> {
    if let Some(api) = api {
        if spec.api_definition_id.is_some() {
            return Err(DomainError::validation(
                "pass only one of api or api_definition_id",
            ));
        }
        if spec.route_config_id.is_some() {
            return Err(DomainError::validation(
                "api cannot be combined with route_config_id scope",
            ));
        }
        let api = api_lifecycle::get_api_definition(pool, team.id, &api)
            .await?
            .ok_or_else(|| DomainError::not_found("api", &api))?;
        spec.api_definition_id = Some(api.id);
    }
    if let Some(api_id) = spec.api_definition_id {
        // Preserve a clear not-found error before the insert path enforces the FK.
        ensure_api_exists(pool, team, api_id).await?;
    }
    Ok(spec)
}

async fn ensure_api_exists(
    pool: &PgPool,
    team: TeamRef,
    api_id: ApiDefinitionId,
) -> DomainResult<()> {
    if api_lifecycle::get_api_definition_by_id(pool, team.id, api_id)
        .await?
        .is_some()
    {
        Ok(())
    } else {
        Err(DomainError::not_found("api", &api_id.to_string()))
    }
}
