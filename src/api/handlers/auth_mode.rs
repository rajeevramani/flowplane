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
    use std::sync::Mutex;

    // Serialize all tests that mutate env vars to prevent races.
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    /// RAII guard that restores env vars on drop (same pattern as tests/common/env_guard.rs).
    struct EnvGuard {
        originals: Vec<(String, Option<String>)>,
    }

    impl EnvGuard {
        fn new() -> Self {
            Self { originals: vec![] }
        }
        fn set(&mut self, key: &str, value: &str) {
            self.originals.push((key.to_string(), std::env::var(key).ok()));
            std::env::set_var(key, value);
        }
        fn remove(&mut self, key: &str) {
            self.originals.push((key.to_string(), std::env::var(key).ok()));
            std::env::remove_var(key);
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, original) in self.originals.iter().rev() {
                match original {
                    Some(val) => std::env::set_var(key, val),
                    None => std::env::remove_var(key),
                }
            }
        }
    }

    #[allow(clippy::await_holding_lock)] // auth_mode_handler is sync-in-async, no real suspend
    #[tokio::test]
    async fn test_auth_mode_handler_returns_prod_by_default() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let mut env = EnvGuard::new();
        env.set("FLOWPLANE_AUTH_MODE", "prod");
        env.remove("FLOWPLANE_ZITADEL_ISSUER");
        env.remove("FLOWPLANE_OIDC_CLIENT_ID");

        let (status, Json(response)) = auth_mode_handler().await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response.auth_mode, "prod");
        assert!(response.oidc_issuer.is_none());
        assert!(response.oidc_client_id.is_none());
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn test_auth_mode_handler_returns_dev_when_set() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let mut env = EnvGuard::new();
        env.set("FLOWPLANE_AUTH_MODE", "dev");

        let (status, Json(response)) = auth_mode_handler().await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response.auth_mode, "dev");
        assert!(response.oidc_issuer.is_none());
        assert!(response.oidc_client_id.is_none());
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn test_auth_mode_prod_includes_oidc_info() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let mut env = EnvGuard::new();
        env.set("FLOWPLANE_AUTH_MODE", "prod");
        env.set("FLOWPLANE_ZITADEL_ISSUER", "https://auth.example.com");
        env.set("FLOWPLANE_OIDC_CLIENT_ID", "test-client-id");

        let (status, Json(response)) = auth_mode_handler().await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response.auth_mode, "prod");
        assert_eq!(response.oidc_issuer.as_deref(), Some("https://auth.example.com"));
        assert_eq!(response.oidc_client_id.as_deref(), Some("test-client-id"));
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
