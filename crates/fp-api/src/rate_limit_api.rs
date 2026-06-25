//! Rate-limit admin surface (feature fpv2-4ht, slice S5). For 1.1.0 this is just the
//! force-repush trigger (spec/01:152); domain/policy/override CRUD is the S3 surface.

use axum::extract::{Extension, State};
use axum::http::StatusCode;
use fp_core::PrincipalCtx;
use fp_domain::{DomainError, RequestId};

use crate::error::ApiError;
use crate::state::AppState;

/// Force an immediate CP→RLS policy reconcile (admin:all governance). The 60 s reconcile is the
/// correctness backstop; this is an ops convenience for "apply now". Returns 202 once the kick
/// is queued; 503 when the RLS sync is not configured.
#[utoipa::path(
    post,
    path = "/api/v1/admin/rls/force-repush",
    tag = "Rate limit",
    responses(
        (status = 202, description = "Reconcile kick accepted"),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 503, body = crate::error::ErrorBody)
    )
)]
pub async fn force_repush(
    State(state): State<AppState>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<StatusCode, ApiError> {
    fp_core::services::rls_sync::authorize_repush(&ctx).map_err(|e| ApiError::new(e, rid))?;
    match &state.rls_repush {
        Some(notify) => {
            notify.notify_one();
            Ok(StatusCode::ACCEPTED)
        }
        None => Err(ApiError::new(
            DomainError::unavailable("rate-limit sync is not configured")
                .with_hint("set FLOWPLANE_RLS_ADMIN_URL to enable CP→RLS policy sync"),
            rid,
        )),
    }
}
