//! Auth mode endpoint — returns the current authentication mode (dev/prod)
//! and OIDC discovery info when in prod mode.

use axum::{http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Auth mode response
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AuthModeResponse {
    /// Current authentication mode ("dev" or "prod")
    #[schema(example = "prod")]
    pub auth_mode: String,

    /// OIDC issuer URL (only present in prod mode)
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = "https://auth.example.com")]
    pub oidc_issuer: Option<String>,

    /// OIDC client ID for CLI authentication (only present in prod mode)
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = "cli-app-client-id")]
    pub oidc_client_id: Option<String>,
}

/// Returns the current authentication mode and OIDC discovery info.
///
/// This endpoint is unauthenticated and intended for CLI and frontends
/// to discover whether the backend is running in dev or prod mode,
/// and where to authenticate.
#[utoipa::path(
    get,
    path = "/api/v1/auth/mode",
    tag = "System",
    responses(
        (status = 200, description = "Current auth mode", body = AuthModeResponse)
    )
)]
pub async fn auth_mode_handler() -> (StatusCode, Json<AuthModeResponse>) {
    let mode = std::env::var("FLOWPLANE_AUTH_MODE").unwrap_or_else(|_| "prod".to_string());

    let (oidc_issuer, oidc_client_id) = if mode == "prod" {
        (
            std::env::var("FLOWPLANE_ZITADEL_ISSUER").ok(),
            std::env::var("FLOWPLANE_OIDC_CLIENT_ID").ok(),
        )
    } else {
        (None, None)
    };

    (StatusCode::OK, Json(AuthModeResponse { auth_mode: mode, oidc_issuer, oidc_client_id }))
}

#[cfg(test)]
mod tests {
    use super::*;

    // NOTE: These tests mutate shared process env vars and MUST run serially.
    // Use `cargo test -- --test-threads=1` or `#[serial_test::serial]` if flaky.

    #[tokio::test]
    async fn test_auth_mode_handler_returns_prod_by_default() {
        // Explicitly set to prod (don't remove — other tests race on this env var)
        let original = std::env::var("FLOWPLANE_AUTH_MODE").ok();
        std::env::set_var("FLOWPLANE_AUTH_MODE", "prod");
        let original_issuer = std::env::var("FLOWPLANE_ZITADEL_ISSUER").ok();
        let original_client = std::env::var("FLOWPLANE_OIDC_CLIENT_ID").ok();
        std::env::remove_var("FLOWPLANE_ZITADEL_ISSUER");
        std::env::remove_var("FLOWPLANE_OIDC_CLIENT_ID");

        let (status, Json(response)) = auth_mode_handler().await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response.auth_mode, "prod");
        // No OIDC info when env vars aren't set
        assert!(response.oidc_issuer.is_none());
        assert!(response.oidc_client_id.is_none());

        // Restore
        match original {
            Some(val) => std::env::set_var("FLOWPLANE_AUTH_MODE", val),
            None => std::env::remove_var("FLOWPLANE_AUTH_MODE"),
        }
        if let Some(val) = original_issuer {
            std::env::set_var("FLOWPLANE_ZITADEL_ISSUER", val);
        }
        if let Some(val) = original_client {
            std::env::set_var("FLOWPLANE_OIDC_CLIENT_ID", val);
        }
    }

    #[tokio::test]
    async fn test_auth_mode_handler_returns_dev_when_set() {
        let original = std::env::var("FLOWPLANE_AUTH_MODE").ok();
        std::env::set_var("FLOWPLANE_AUTH_MODE", "dev");

        let (status, Json(response)) = auth_mode_handler().await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response.auth_mode, "dev");
        // No OIDC info in dev mode
        assert!(response.oidc_issuer.is_none());
        assert!(response.oidc_client_id.is_none());

        // Restore
        match original {
            Some(val) => std::env::set_var("FLOWPLANE_AUTH_MODE", val),
            None => std::env::remove_var("FLOWPLANE_AUTH_MODE"),
        }
    }

    #[tokio::test]
    async fn test_auth_mode_prod_includes_oidc_info() {
        let original_mode = std::env::var("FLOWPLANE_AUTH_MODE").ok();
        let original_issuer = std::env::var("FLOWPLANE_ZITADEL_ISSUER").ok();
        let original_client = std::env::var("FLOWPLANE_OIDC_CLIENT_ID").ok();

        std::env::set_var("FLOWPLANE_AUTH_MODE", "prod");
        std::env::set_var("FLOWPLANE_ZITADEL_ISSUER", "https://auth.example.com");
        std::env::set_var("FLOWPLANE_OIDC_CLIENT_ID", "test-client-id");

        let (status, Json(response)) = auth_mode_handler().await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response.auth_mode, "prod");
        assert_eq!(response.oidc_issuer.as_deref(), Some("https://auth.example.com"));
        assert_eq!(response.oidc_client_id.as_deref(), Some("test-client-id"));

        // Restore
        match original_mode {
            Some(val) => std::env::set_var("FLOWPLANE_AUTH_MODE", val),
            None => std::env::remove_var("FLOWPLANE_AUTH_MODE"),
        }
        match original_issuer {
            Some(val) => std::env::set_var("FLOWPLANE_ZITADEL_ISSUER", val),
            None => std::env::remove_var("FLOWPLANE_ZITADEL_ISSUER"),
        }
        match original_client {
            Some(val) => std::env::set_var("FLOWPLANE_OIDC_CLIENT_ID", val),
            None => std::env::remove_var("FLOWPLANE_OIDC_CLIENT_ID"),
        }
    }

    #[test]
    fn test_auth_mode_response_serialization() {
        let response = AuthModeResponse {
            auth_mode: "dev".to_string(),
            oidc_issuer: None,
            oidc_client_id: None,
        };
        let json = serde_json::to_value(&response).expect("serialize");
        assert_eq!(json, serde_json::json!({"auth_mode": "dev"}));
    }

    #[test]
    fn test_auth_mode_response_prod_serialization() {
        let response = AuthModeResponse {
            auth_mode: "prod".to_string(),
            oidc_issuer: Some("https://auth.example.com".to_string()),
            oidc_client_id: Some("cli-client".to_string()),
        };
        let json = serde_json::to_value(&response).expect("serialize");
        assert_eq!(json["auth_mode"], "prod");
        assert_eq!(json["oidc_issuer"], "https://auth.example.com");
        assert_eq!(json["oidc_client_id"], "cli-client");
    }
}
