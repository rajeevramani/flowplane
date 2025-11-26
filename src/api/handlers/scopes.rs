//! Scope management endpoints for scope discovery and validation
//!
//! This module provides endpoints for:
//! - Listing available scopes for UI display
//! - Listing all scopes for admin management

use axum::{http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth::scope_registry::{get_scope_registry, is_scope_registry_initialized};
use crate::storage::repositories::ScopeDefinition;

/// Response for listing scopes
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListScopesResponse {
    /// List of available scopes
    pub scopes: Vec<ScopeDefinition>,
    /// Total count of scopes
    pub count: usize,
}

/// List available scopes for UI
///
/// Returns all UI-visible scopes that can be assigned to tokens.
/// This endpoint is public (no authentication required) since users need
/// to see available scopes when creating or viewing tokens.
///
/// Scopes that are marked as not visible in UI (like admin:all) are excluded.
#[utoipa::path(
    get,
    path = "/api/v1/scopes",
    tag = "scopes",
    responses(
        (status = 200, description = "List of available scopes", body = ListScopesResponse),
        (status = 503, description = "Scope registry not initialized")
    )
)]
pub async fn list_scopes_handler() -> Result<Json<ListScopesResponse>, (StatusCode, String)> {
    if !is_scope_registry_initialized() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "Scope registry not initialized".to_string(),
        ));
    }

    let registry = get_scope_registry();
    let scopes = registry.get_ui_scopes();
    let count = scopes.len();

    Ok(Json(ListScopesResponse { scopes, count }))
}

/// List all scopes (admin)
///
/// Returns all scopes including those not visible in UI (like admin:all).
/// This endpoint requires admin access and is useful for:
/// - Auditing available permissions
/// - Debugging scope issues
/// - Managing scope visibility
#[utoipa::path(
    get,
    path = "/api/v1/admin/scopes",
    tag = "scopes",
    responses(
        (status = 200, description = "List of all scopes", body = ListScopesResponse),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden - admin access required"),
        (status = 503, description = "Scope registry not initialized")
    ),
    security(
        ("bearer" = [])
    )
)]
pub async fn list_all_scopes_handler() -> Result<Json<ListScopesResponse>, (StatusCode, String)> {
    // Note: Authorization is handled by the auth middleware
    // This handler just returns all scopes

    if !is_scope_registry_initialized() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "Scope registry not initialized".to_string(),
        ));
    }

    let registry = get_scope_registry();
    let scopes = registry.get_all_scopes();
    let count = scopes.len();

    Ok(Json(ListScopesResponse { scopes, count }))
}

#[cfg(test)]
mod tests {
    // Integration tests for scope endpoints would require a full test setup
    // with database and scope registry initialization.
    // Unit tests for the response structure:

    use super::*;

    #[test]
    fn test_list_scopes_response_serialization() {
        let response = ListScopesResponse { scopes: vec![], count: 0 };

        let json = serde_json::to_string(&response).expect("serialize");
        assert!(json.contains("\"scopes\""));
        assert!(json.contains("\"count\""));
    }
}
