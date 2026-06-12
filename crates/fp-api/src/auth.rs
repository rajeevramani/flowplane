//! Authentication middleware: bearer token → validated claims → JIT user → loaded
//! [`PrincipalCtx`] in request extensions. Authn failures are audited best-effort
//! (spec/08a §6 — v1 never audited them).

use crate::error::ApiError;
use crate::state::AppState;
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use fp_core::{GrantSet, PrincipalCtx};
use fp_domain::{DomainError, ErrorCode, RequestId};
use fp_storage::repos::{audit, identity};

fn bearer(headers: &axum::http::HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

pub async fn authenticate(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let rid = request
        .extensions()
        .get::<RequestId>()
        .copied()
        .unwrap_or_else(RequestId::generate);

    let Some(validator) = state.validator.clone() else {
        return ApiError::new(
            DomainError::unavailable("authentication is not configured on this server")
                .with_hint("set FLOWPLANE_OIDC_ISSUER/FLOWPLANE_OIDC_AUDIENCE (or dev mode)"),
            rid,
        )
        .into_response();
    };

    let Some(token) = bearer(request.headers()) else {
        return ApiError::new(
            DomainError::new(ErrorCode::Unauthorized, "missing bearer token")
                .with_hint("authenticate with `flowplane auth login` and retry"),
            rid,
        )
        .into_response();
    };

    let claims = match validator.validate(token).await {
        Ok(claims) => claims,
        Err(e) => {
            audit::record_best_effort(
                &state.pool,
                &audit::AuditEntry {
                    request_id: Some(rid),
                    actor_type: audit::ActorType::Anonymous,
                    actor_id: None,
                    actor_label: String::new(),
                    surface: audit::Surface::Rest,
                    action: "authn.failed".into(),
                    resource: request.uri().path().to_string(),
                    org_id: None,
                    team_id: None,
                    outcome: audit::Outcome::Failure,
                    detail: serde_json::json!({ "code": e.code.as_str() }),
                },
            )
            .await;
            metrics::counter!("fp_authn_failures_total").increment(1);
            return ApiError::new(e, rid).into_response();
        }
    };

    // JIT provisioning + principal load (spec/05 §1).
    let principal = async {
        identity::upsert_user_by_subject(
            &state.pool,
            &claims.subject,
            claims.email.as_deref().unwrap_or(""),
            claims.name.as_deref().unwrap_or(""),
        )
        .await?;
        identity::load_principal(&state.pool, &claims.subject).await
    }
    .await;

    match principal {
        Ok(Some(loaded)) => {
            let ctx = PrincipalCtx::User {
                user_id: loaded.user_id,
                platform_admin: loaded.platform_admin,
                org: loaded.org,
                grants: GrantSet::new(loaded.grants),
            };
            request.extensions_mut().insert(ctx);
            next.run(request).await
        }
        Ok(None) => ApiError::new(
            DomainError::new(ErrorCode::Unauthorized, "account is not available"),
            rid,
        )
        .into_response(),
        Err(e) => ApiError::new(e, rid).into_response(),
    }
}
