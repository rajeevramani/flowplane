//! Authentication middleware: bearer token → validated claims → JIT user → loaded
//! [`PrincipalCtx`] in request extensions. Authn failures are audited best-effort
//! (spec/08a §6 — v1 never audited them).

use crate::error::ApiError;
use crate::state::AppState;
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use fp_core::{GrantSet, PrincipalCtx};
use fp_domain::{DomainError, DomainResult, ErrorCode, OrgId, OrgRole, RequestId};
use fp_storage::repos::{audit, identity};
use std::str::FromStr;

/// Header carrying the active-org selector (D-014): an org name or UUID.
const ORG_SELECTOR_HEADER: &str = "x-flowplane-org";

fn bearer(headers: &axum::http::HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

/// Resolve the validated active org for a request (D-014 policy).
///
/// - selector present → must resolve to an org the user actively belongs to; else fail closed;
/// - absent + exactly one non-platform membership → use it;
/// - absent + zero or ≥2 candidates → no active org.
///
/// The platform org is never an inferable/selectable tenant context. Returns
/// `(active_org, selector_required)`: `selector_required` is true only when the absence is
/// because the caller must name an org (ambiguous, or an unresolvable selector), not when they
/// simply have no org access.
async fn resolve_active_org(
    state: &AppState,
    loaded: &identity::LoadedPrincipal,
    headers: &axum::http::HeaderMap,
) -> DomainResult<(Option<(OrgId, OrgRole)>, bool)> {
    let candidates: Vec<(OrgId, OrgRole)> = loaded
        .memberships
        .iter()
        .copied()
        .filter(|(org_id, _)| Some(*org_id) != loaded.platform_org_id)
        .collect();

    let selector = headers
        .get(ORG_SELECTOR_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let selected_id = match selector {
        Some(sel) => match OrgId::from_str(sel) {
            Ok(id) => Some(id),
            Err(_) => identity::find_active_org_id_by_name(&state.pool, sel).await?,
        },
        None => None,
    };
    Ok(pick_active_org(
        &candidates,
        selected_id,
        selector.is_some(),
    ))
}

/// The pure D-014 selection rule (no IO), given the caller's non-platform candidate orgs, the
/// selector's resolved org id (if a selector was present and resolvable to *some* id), and
/// whether a selector was present at all.
fn pick_active_org(
    candidates: &[(OrgId, OrgRole)],
    selected_id: Option<OrgId>,
    selector_present: bool,
) -> (Option<(OrgId, OrgRole)>, bool) {
    if selector_present {
        // The selector must resolve to an org the caller is actually a member of.
        let active = selected_id.and_then(|id| candidates.iter().find(|(o, _)| *o == id).copied());
        // Given but unresolvable/non-member → fail closed; tell them to pick a valid org.
        return (active, active.is_none());
    }
    match candidates {
        [one] => (Some(*one), false),
        [] => (None, false), // genuinely no tenant org access
        _ => (None, true),   // ambiguous: must select
    }
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
            // D-014: resolve the single validated active org from the membership set + the
            // request's org selector. Never an implicit "first membership wins".
            let (org, org_selector_required) =
                match resolve_active_org(&state, &loaded, request.headers()).await {
                    Ok(resolved) => resolved,
                    Err(e) => return ApiError::new(e, rid).into_response(),
                };
            let ctx = PrincipalCtx::User {
                user_id: loaded.user_id,
                platform_admin: loaded.platform_admin,
                org,
                org_selector_required,
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

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use fp_domain::OrgId;

    fn org(role: OrgRole) -> (OrgId, OrgRole) {
        (OrgId::generate(), role)
    }

    #[test]
    fn selection_policy_matrix() {
        let a = org(OrgRole::Admin);
        let b = org(OrgRole::Member);

        // No selector, exactly one candidate → infer it.
        assert_eq!(pick_active_org(&[a], None, false), (Some(a), false));
        // No selector, zero candidates → no active org, NOT a selector problem.
        assert_eq!(pick_active_org(&[], None, false), (None, false));
        // No selector, multiple candidates → fail closed, selector required.
        assert_eq!(pick_active_org(&[a, b], None, false), (None, true));

        // Selector resolves to a member org → that org (even with multiple candidates).
        assert_eq!(pick_active_org(&[a, b], Some(a.0), true), (Some(a), false));
        // Selector present but unresolvable (None id) → fail closed, selector required.
        assert_eq!(pick_active_org(&[a, b], None, true), (None, true));
        // Selector resolves to an org the caller is NOT a member of → fail closed.
        let stranger = OrgId::generate();
        assert_eq!(pick_active_org(&[a, b], Some(stranger), true), (None, true));
    }
}
