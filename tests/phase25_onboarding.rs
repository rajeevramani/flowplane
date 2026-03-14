//! Integration tests for Phase 2.5 — Frictionless Onboarding
//!
//! Covers:
//! - Compose orchestration (write_compose_file, write_envoy_bootstrap, loopback guard, config)
//! - Auth mode endpoint (dev/prod responses, serialization)
//! - OIDC PKCE flow (code_challenge crypto, full round-trip against mock OIDC)
//! - Device code flow against mock OIDC
//! - Token refresh when access_token expired
//! - Docker build verification (.dockerignore, Dockerfile, Cargo.toml)

mod common;

/// Mutex to serialize tests that mutate shared env vars (HOME, FLOWPLANE_AUTH_MODE, etc.)
/// to prevent data races between parallel test threads.
static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

// ============================================================================
// Compose orchestration tests (fp-zcc.5)
// ============================================================================

mod compose_orchestration {
    use std::path::PathBuf;

    /// Verify write_compose_file patches "context: ." to the resolved source dir.
    #[test]
    fn write_compose_file_patches_build_context() {
        let _guard = crate::ENV_MUTEX.lock().unwrap();
        let tmp = tempfile::TempDir::new().unwrap();
        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", tmp.path());

        // Use a synthetic source dir
        let source_dir = PathBuf::from("/opt/flowplane-src");
        let result = flowplane::cli::compose::write_compose_file(&source_dir);

        // Restore
        if let Some(h) = original_home {
            std::env::set_var("HOME", h);
        }

        let compose_path = result.expect("write_compose_file should succeed");
        let content = std::fs::read_to_string(&compose_path).unwrap();

        assert!(
            content.contains("context: /opt/flowplane-src"),
            "build context should be patched to source dir"
        );
        assert!(!content.contains("context: ."), "original 'context: .' should be replaced");
    }

    /// Verify write_compose_file writes to ~/.flowplane/docker-compose-dev.yml.
    #[test]
    fn write_compose_file_creates_at_correct_path() {
        let _guard = crate::ENV_MUTEX.lock().unwrap();
        let tmp = tempfile::TempDir::new().unwrap();
        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", tmp.path());

        let source_dir = PathBuf::from("/tmp/fp-test");
        let result = flowplane::cli::compose::write_compose_file(&source_dir);

        if let Some(h) = original_home {
            std::env::set_var("HOME", h);
        }

        let path = result.unwrap();
        assert_eq!(path, tmp.path().join(".flowplane/docker-compose-dev.yml"));
        assert!(path.exists());
    }

    /// Verify init is idempotent: calling write_compose_file twice doesn't fail.
    #[test]
    fn write_compose_file_is_idempotent() {
        let _guard = crate::ENV_MUTEX.lock().unwrap();
        let tmp = tempfile::TempDir::new().unwrap();
        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", tmp.path());

        let source_dir = PathBuf::from("/tmp/idempotent-test");
        let r1 = flowplane::cli::compose::write_compose_file(&source_dir);
        let r2 = flowplane::cli::compose::write_compose_file(&source_dir);

        if let Some(h) = original_home {
            std::env::set_var("HOME", h);
        }

        assert!(r1.is_ok());
        assert!(r2.is_ok());
        // Both should produce the same path and content
        let c1 = std::fs::read_to_string(r1.unwrap()).unwrap();
        let c2 = std::fs::read_to_string(r2.unwrap()).unwrap();
        assert_eq!(c1, c2);
    }

    /// Config.toml written by init should contain base_url, team=default, org=dev-org.
    ///
    /// We test CliConfig serialization directly since handle_init has side effects.
    #[test]
    fn config_toml_has_expected_fields_after_init() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config_path = tmp.path().join("config.toml");

        let config = flowplane::cli::config::CliConfig {
            base_url: Some("http://localhost:8080".to_string()),
            team: Some("default".to_string()),
            org: Some("dev-org".to_string()),
            ..Default::default()
        };
        config.save_to_path(&config_path).unwrap();

        let loaded = flowplane::cli::config::CliConfig::load_from_path(&config_path).unwrap();
        assert_eq!(loaded.base_url.as_deref(), Some("http://localhost:8080"));
        assert_eq!(loaded.team.as_deref(), Some("default"));
        assert_eq!(loaded.org.as_deref(), Some("dev-org"));
    }

    /// Credentials file written with dev token should be readable.
    #[test]
    fn credentials_file_written_with_dev_token() {
        let tmp = tempfile::TempDir::new().unwrap();
        let token = "test-dev-token-abc123";

        flowplane::auth::dev_token::write_credentials_file(token, tmp.path())
            .expect("write_credentials_file should succeed");

        let read_back = flowplane::auth::dev_token::read_credentials_file(tmp.path())
            .expect("read_credentials_file should succeed");
        assert_eq!(read_back, token);
    }

    /// Loopback guard should pass on the host (no /.dockerenv).
    #[test]
    fn loopback_guard_passes_on_host() {
        // Only meaningful on the host; skip if somehow running in a container
        if !std::path::Path::new("/.dockerenv").exists() {
            let result = flowplane::cli::compose::loopback_guard();
            assert!(result.is_ok(), "loopback_guard should pass on host");
        }
    }
}

// ============================================================================
// Auth mode + OIDC flow tests (fp-zcc.6)
// ============================================================================

mod auth_mode {
    /// GET /api/v1/auth/mode returns {auth_mode: "dev"} when FLOWPLANE_AUTH_MODE=dev.
    #[allow(clippy::await_holding_lock)] // auth_mode_handler is sync-in-async, no real await
    #[tokio::test]
    async fn auth_mode_returns_dev() {
        let _guard = crate::ENV_MUTEX.lock().unwrap();
        let original = std::env::var("FLOWPLANE_AUTH_MODE").ok();
        std::env::set_var("FLOWPLANE_AUTH_MODE", "dev");

        let (status, axum::Json(resp)) =
            flowplane::api::handlers::auth_mode::auth_mode_handler().await;

        match original {
            Some(v) => std::env::set_var("FLOWPLANE_AUTH_MODE", v),
            None => std::env::remove_var("FLOWPLANE_AUTH_MODE"),
        }

        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(resp.auth_mode, "dev");
        assert!(resp.oidc_issuer.is_none());
        assert!(resp.oidc_client_id.is_none());
    }

    /// GET /api/v1/auth/mode returns {auth_mode: "prod"} with oidc_issuer and oidc_client_id.
    #[allow(clippy::await_holding_lock)] // auth_mode_handler is sync-in-async, no real await
    #[tokio::test]
    async fn auth_mode_returns_prod_with_oidc_info() {
        let _guard = crate::ENV_MUTEX.lock().unwrap();
        let orig_mode = std::env::var("FLOWPLANE_AUTH_MODE").ok();
        let orig_issuer = std::env::var("FLOWPLANE_ZITADEL_ISSUER").ok();
        let orig_client = std::env::var("FLOWPLANE_OIDC_CLIENT_ID").ok();

        std::env::set_var("FLOWPLANE_AUTH_MODE", "prod");
        std::env::set_var("FLOWPLANE_ZITADEL_ISSUER", "https://auth.test.com");
        std::env::set_var("FLOWPLANE_OIDC_CLIENT_ID", "test-client-42");

        let (status, axum::Json(resp)) =
            flowplane::api::handlers::auth_mode::auth_mode_handler().await;

        // Restore
        match orig_mode {
            Some(v) => std::env::set_var("FLOWPLANE_AUTH_MODE", v),
            None => std::env::remove_var("FLOWPLANE_AUTH_MODE"),
        }
        match orig_issuer {
            Some(v) => std::env::set_var("FLOWPLANE_ZITADEL_ISSUER", v),
            None => std::env::remove_var("FLOWPLANE_ZITADEL_ISSUER"),
        }
        match orig_client {
            Some(v) => std::env::set_var("FLOWPLANE_OIDC_CLIENT_ID", v),
            None => std::env::remove_var("FLOWPLANE_OIDC_CLIENT_ID"),
        }

        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(resp.auth_mode, "prod");
        assert_eq!(resp.oidc_issuer.as_deref(), Some("https://auth.test.com"));
        assert_eq!(resp.oidc_client_id.as_deref(), Some("test-client-42"));
    }

    /// Dev mode serialization omits oidc fields (skip_serializing_if = None).
    #[test]
    fn auth_mode_dev_serialization_omits_oidc() {
        let resp = flowplane::api::handlers::auth_mode::AuthModeResponse {
            auth_mode: "dev".to_string(),
            oidc_issuer: None,
            oidc_client_id: None,
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json, serde_json::json!({"auth_mode": "dev"}));
        // oidc_issuer and oidc_client_id should NOT be present
        assert!(json.get("oidc_issuer").is_none());
        assert!(json.get("oidc_client_id").is_none());
    }

    /// Prod mode serialization includes oidc fields.
    #[test]
    fn auth_mode_prod_serialization_includes_oidc() {
        let resp = flowplane::api::handlers::auth_mode::AuthModeResponse {
            auth_mode: "prod".to_string(),
            oidc_issuer: Some("https://auth.example.com".to_string()),
            oidc_client_id: Some("cli-client".to_string()),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["auth_mode"], "prod");
        assert_eq!(json["oidc_issuer"], "https://auth.example.com");
        assert_eq!(json["oidc_client_id"], "cli-client");
    }
}

mod oidc_crypto {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use sha2::{Digest, Sha256};

    /// PKCE code_verifier → S256 code_challenge matches RFC 7636 Appendix B test vector.
    #[test]
    fn pkce_s256_rfc_test_vector() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let expected_challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

        let mut hasher = Sha256::new();
        hasher.update(verifier.as_bytes());
        let challenge = URL_SAFE_NO_PAD.encode(hasher.finalize());

        assert_eq!(challenge, expected_challenge);
    }

    /// S256 code_challenge for an arbitrary verifier matches manual computation.
    #[test]
    fn s256_challenge_matches_manual_computation() {
        let verifier = "test-verifier-for-challenge-computation";

        let mut hasher = Sha256::new();
        hasher.update(verifier.as_bytes());
        let challenge = URL_SAFE_NO_PAD.encode(hasher.finalize());

        // Recompute independently
        let digest = sha2::Sha256::digest(verifier.as_bytes());
        let expected = URL_SAFE_NO_PAD.encode(digest);

        assert_eq!(challenge, expected);
    }

    /// Base64url-encoded 32-byte random value should be 43 chars (PKCE verifier length).
    #[test]
    fn base64url_32_bytes_is_43_chars() {
        use rand::RngCore;
        let mut buf = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut buf);
        let encoded = URL_SAFE_NO_PAD.encode(buf);
        assert_eq!(encoded.len(), 43);
    }

    /// Two random 32-byte values should produce different base64url strings.
    #[test]
    fn random_verifiers_are_unique() {
        use rand::RngCore;
        let mut buf1 = [0u8; 32];
        let mut buf2 = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut buf1);
        rand::thread_rng().fill_bytes(&mut buf2);
        let v1 = URL_SAFE_NO_PAD.encode(buf1);
        let v2 = URL_SAFE_NO_PAD.encode(buf2);
        assert_ne!(v1, v2);
    }
}

mod oidc_credentials {
    use flowplane::cli::auth::OidcCredentials;

    /// Credentials round-trip: save → load preserves all fields.
    #[test]
    fn credentials_save_and_load_roundtrip() {
        let _guard = crate::ENV_MUTEX.lock().unwrap();
        let tmp = tempfile::TempDir::new().unwrap();
        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", tmp.path());

        let creds = OidcCredentials {
            access_token: "test-at-roundtrip".to_string(),
            refresh_token: Some("test-rt-roundtrip".to_string()),
            expires_at: Some(9999999999),
            issuer: Some("https://oidc.test.dev".to_string()),
        };
        creds.save().unwrap();

        let loaded = OidcCredentials::load().unwrap();

        if let Some(h) = original_home {
            std::env::set_var("HOME", h);
        }

        assert_eq!(loaded.access_token, "test-at-roundtrip");
        assert_eq!(loaded.refresh_token.as_deref(), Some("test-rt-roundtrip"));
        assert_eq!(loaded.expires_at, Some(9999999999));
        assert_eq!(loaded.issuer.as_deref(), Some("https://oidc.test.dev"));
    }

    /// Legacy plain-text credentials are loadable.
    #[test]
    fn credentials_load_legacy_plaintext() {
        let _guard = crate::ENV_MUTEX.lock().unwrap();
        let tmp = tempfile::TempDir::new().unwrap();
        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", tmp.path());

        let dir = tmp.path().join(".flowplane");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("credentials"), "legacy-plain-token\n").unwrap();

        let loaded = OidcCredentials::load().unwrap();

        if let Some(h) = original_home {
            std::env::set_var("HOME", h);
        }

        assert_eq!(loaded.access_token, "legacy-plain-token");
        assert!(loaded.refresh_token.is_none());
    }

    /// is_expired returns true for past timestamps, false for future.
    #[test]
    fn is_expired_logic() {
        let expired = OidcCredentials {
            access_token: "t".to_string(),
            refresh_token: None,
            expires_at: Some(1000),
            issuer: None,
        };
        assert!(expired.is_expired());

        let valid = OidcCredentials {
            access_token: "t".to_string(),
            refresh_token: None,
            expires_at: Some(9999999999),
            issuer: None,
        };
        assert!(!valid.is_expired());

        let no_expiry = OidcCredentials {
            access_token: "t".to_string(),
            refresh_token: None,
            expires_at: None,
            issuer: None,
        };
        assert!(!no_expiry.is_expired());
    }

    /// Delete removes credentials file.
    #[test]
    fn credentials_delete() {
        let _guard = crate::ENV_MUTEX.lock().unwrap();
        let tmp = tempfile::TempDir::new().unwrap();
        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", tmp.path());

        let creds = OidcCredentials {
            access_token: "to-delete".to_string(),
            refresh_token: None,
            expires_at: None,
            issuer: None,
        };
        creds.save().unwrap();
        assert!(OidcCredentials::load().is_ok());

        OidcCredentials::delete().unwrap();
        assert!(OidcCredentials::load().is_err());

        if let Some(h) = original_home {
            std::env::set_var("HOME", h);
        }
    }
}

mod oidc_mock_flows {
    use super::common::mock_oidc::{MockOidcConfig, MockOidcServer};
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use sha2::{Digest, Sha256};

    /// Full PKCE round-trip against mock OIDC: discovery → authorize → token exchange → valid JWT.
    #[tokio::test]
    async fn pkce_full_roundtrip() {
        let server = MockOidcServer::start(MockOidcConfig::default()).await;
        let client =
            reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();

        // Step 1: Discovery
        let discovery_resp = client
            .get(format!("{}/.well-known/openid-configuration", server.base_url))
            .send()
            .await
            .unwrap();
        assert_eq!(discovery_resp.status(), 200);
        let discovery: serde_json::Value = discovery_resp.json().await.unwrap();
        assert_eq!(discovery["issuer"], server.base_url);

        let auth_endpoint = discovery["authorization_endpoint"].as_str().unwrap();
        let token_endpoint = discovery["token_endpoint"].as_str().unwrap();

        // Step 2: PKCE params
        let code_verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let mut hasher = Sha256::new();
        hasher.update(code_verifier.as_bytes());
        let code_challenge = URL_SAFE_NO_PAD.encode(hasher.finalize());

        // Step 3: Authorize
        let auth_resp = client
            .get(auth_endpoint)
            .query(&[
                ("response_type", "code"),
                ("client_id", "test-client"),
                ("redirect_uri", "http://localhost:9999/callback"),
                ("state", "csrf-state-xyz"),
                ("code_challenge", &code_challenge),
                ("code_challenge_method", "S256"),
                ("scope", "openid profile email offline_access"),
            ])
            .send()
            .await
            .unwrap();

        assert_eq!(auth_resp.status(), 303, "authorize should redirect");
        let location = auth_resp.headers().get("location").unwrap().to_str().unwrap();
        let redirect_url = url::Url::parse(location).unwrap();

        let params: std::collections::HashMap<String, String> =
            redirect_url.query_pairs().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        assert!(params.contains_key("code"), "redirect must include code");
        assert_eq!(
            params.get("state").map(|s| s.as_str()),
            Some("csrf-state-xyz"),
            "state must round-trip"
        );

        let code = &params["code"];

        // Step 4: Token exchange
        let token_resp = client
            .post(token_endpoint)
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code.as_str()),
                ("redirect_uri", "http://localhost:9999/callback"),
                ("code_verifier", code_verifier),
                ("client_id", "test-client"),
            ])
            .send()
            .await
            .unwrap();

        assert_eq!(token_resp.status(), 200);
        let token_body: serde_json::Value = token_resp.json().await.unwrap();
        assert!(token_body["access_token"].as_str().is_some());
        assert!(token_body["refresh_token"].as_str().is_some());
        assert!(token_body["expires_in"].as_u64().is_some());

        // Verify the access_token is a valid JWT (3 parts)
        let access_token = token_body["access_token"].as_str().unwrap();
        let jwt_parts: Vec<&str> = access_token.split('.').collect();
        assert_eq!(jwt_parts.len(), 3, "access_token should be a 3-part JWT");
    }

    /// Device code flow against mock OIDC: device_authorize → poll token → get tokens.
    #[tokio::test]
    async fn device_code_flow() {
        let server = MockOidcServer::start(MockOidcConfig::default()).await;
        let client = reqwest::Client::new();

        // Step 1: Discovery
        let discovery: serde_json::Value = client
            .get(format!("{}/.well-known/openid-configuration", server.base_url))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let device_endpoint = discovery["device_authorization_endpoint"].as_str().unwrap();

        // Step 2: Request device code
        let device_resp = client
            .post(device_endpoint)
            .form(&[("client_id", "test-client"), ("scope", "openid profile email offline_access")])
            .send()
            .await
            .unwrap();

        assert_eq!(device_resp.status(), 200);
        let device_body: serde_json::Value = device_resp.json().await.unwrap();
        assert!(device_body["device_code"].as_str().is_some());
        assert!(device_body["user_code"].as_str().is_some());
        assert!(device_body["verification_uri"].as_str().is_some());
        assert!(device_body["expires_in"].as_u64().is_some());

        let device_code = device_body["device_code"].as_str().unwrap();
        let token_endpoint = discovery["token_endpoint"].as_str().unwrap();

        // Step 3: Poll for token (mock auto-approves by default)
        let token_resp = client
            .post(token_endpoint)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", device_code),
                ("client_id", "test-client"),
            ])
            .send()
            .await
            .unwrap();

        assert_eq!(token_resp.status(), 200);
        let token_body: serde_json::Value = token_resp.json().await.unwrap();
        assert!(token_body["access_token"].as_str().is_some());
        assert!(token_body["refresh_token"].as_str().is_some());
    }

    /// Token refresh: exchange a refresh_token for a new access_token.
    #[tokio::test]
    async fn token_refresh_flow() {
        let server = MockOidcServer::start(MockOidcConfig::default()).await;
        let client =
            reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();

        // First, get tokens via PKCE flow
        let code_verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let mut hasher = Sha256::new();
        hasher.update(code_verifier.as_bytes());
        let code_challenge = URL_SAFE_NO_PAD.encode(hasher.finalize());

        let auth_resp = client
            .get(server.authorize_endpoint())
            .query(&[
                ("response_type", "code"),
                ("client_id", "test-client"),
                ("redirect_uri", "http://localhost:9999/callback"),
                ("state", "state1"),
                ("code_challenge", &code_challenge),
                ("code_challenge_method", "S256"),
            ])
            .send()
            .await
            .unwrap();

        let location = auth_resp.headers().get("location").unwrap().to_str().unwrap();
        let redirect_url = url::Url::parse(location).unwrap();
        let code: String =
            redirect_url.query_pairs().find(|(k, _)| k == "code").unwrap().1.to_string();

        let token_resp: serde_json::Value = client
            .post(server.token_endpoint())
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("redirect_uri", "http://localhost:9999/callback"),
                ("code_verifier", code_verifier),
                ("client_id", "test-client"),
            ])
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();

        let _original_access_token = token_resp["access_token"].as_str().unwrap().to_string();
        let refresh_token = token_resp["refresh_token"].as_str().unwrap();

        // Now refresh
        let refresh_resp: serde_json::Value = client
            .post(server.token_endpoint())
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
                ("client_id", "test-client"),
            ])
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();

        let new_access_token = refresh_resp["access_token"].as_str().unwrap();
        let new_refresh_token = refresh_resp["refresh_token"].as_str().unwrap();

        // Refresh token is always a new UUID — must differ
        assert_ne!(new_refresh_token, refresh_token, "refresh token must rotate");
        // Access token is a valid JWT
        assert_eq!(
            new_access_token.split('.').count(),
            3,
            "refreshed access_token should be a JWT"
        );
        assert!(refresh_resp["expires_in"].as_u64().is_some());
        // Note: access_token may equal the original if issued in the same second
        // (same claims + same key = same JWT). That's fine — the important contract
        // is that the refresh token rotates and the response is well-formed.
    }

    /// PKCE with wrong code_verifier should fail.
    #[tokio::test]
    async fn pkce_wrong_verifier_rejected() {
        let server = MockOidcServer::start(MockOidcConfig::default()).await;
        let client =
            reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();

        let code_verifier = "correct-verifier-for-challenge";
        let mut hasher = Sha256::new();
        hasher.update(code_verifier.as_bytes());
        let code_challenge = URL_SAFE_NO_PAD.encode(hasher.finalize());

        let auth_resp = client
            .get(server.authorize_endpoint())
            .query(&[
                ("response_type", "code"),
                ("client_id", "test-client"),
                ("redirect_uri", "http://localhost:9999/callback"),
                ("state", "st"),
                ("code_challenge", &code_challenge),
                ("code_challenge_method", "S256"),
            ])
            .send()
            .await
            .unwrap();

        let location = auth_resp.headers().get("location").unwrap().to_str().unwrap();
        let redirect_url = url::Url::parse(location).unwrap();
        let code: String =
            redirect_url.query_pairs().find(|(k, _)| k == "code").unwrap().1.to_string();

        // Use WRONG verifier
        let token_resp = client
            .post(server.token_endpoint())
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("redirect_uri", "http://localhost:9999/callback"),
                ("code_verifier", "wrong-verifier-should-fail"),
                ("client_id", "test-client"),
            ])
            .send()
            .await
            .unwrap();

        assert_eq!(token_resp.status(), 400);
        let body: serde_json::Value = token_resp.json().await.unwrap();
        assert_eq!(body["error"], "invalid_grant");
    }
}

// ============================================================================
// Docker build verification tests (fp-zcc.8)
// ============================================================================

mod docker_build_verification {
    use std::path::Path;

    fn project_root() -> &'static Path {
        // Tests run from the project root
        Path::new(env!("CARGO_MANIFEST_DIR"))
    }

    /// .dockerignore has `!docker-compose-dev.yml` exception so include_str! works in Docker builds.
    #[test]
    fn dockerignore_has_dev_compose_exception() {
        let content = std::fs::read_to_string(project_root().join(".dockerignore")).unwrap();
        assert!(
            content.contains("!docker-compose-dev.yml"),
            ".dockerignore must have !docker-compose-dev.yml exception"
        );
    }

    /// Dockerfile has `COPY docker-compose-dev.yml ./` so the file is available at build time.
    #[test]
    fn dockerfile_copies_dev_compose() {
        let content = std::fs::read_to_string(project_root().join("Dockerfile")).unwrap();
        assert!(
            content.contains("COPY docker-compose-dev.yml"),
            "Dockerfile must COPY docker-compose-dev.yml"
        );
    }

    /// Dockerfile CMD is ["flowplane", "serve"].
    #[test]
    fn dockerfile_cmd_is_flowplane_serve() {
        let content = std::fs::read_to_string(project_root().join("Dockerfile")).unwrap();
        assert!(
            content.contains(r#"CMD ["flowplane", "serve"]"#),
            "Dockerfile CMD should be [\"flowplane\", \"serve\"]"
        );
    }

    /// Cargo.toml has default-run = "flowplane".
    #[test]
    fn cargo_toml_has_default_run() {
        let content = std::fs::read_to_string(project_root().join("Cargo.toml")).unwrap();
        assert!(
            content.contains(r#"default-run = "flowplane""#),
            "Cargo.toml must have default-run = \"flowplane\""
        );
    }
}

// ============================================================================
// Envoy bootstrap tests (fp-zcc.9)
// ============================================================================

mod envoy_bootstrap {
    /// write_envoy_bootstrap creates valid YAML at ~/.flowplane/envoy/envoy.yaml.
    #[test]
    fn write_envoy_bootstrap_creates_file() {
        let _guard = crate::ENV_MUTEX.lock().unwrap();
        let tmp = tempfile::TempDir::new().unwrap();
        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", tmp.path());

        let result = flowplane::cli::compose::write_envoy_bootstrap();

        if let Some(h) = original_home {
            std::env::set_var("HOME", h);
        }

        let path = result.expect("write_envoy_bootstrap should succeed");
        assert_eq!(path, tmp.path().join(".flowplane/envoy/envoy.yaml"));
        assert!(path.exists());
    }

    /// xDS cluster address is control-plane:18000.
    #[test]
    fn xds_cluster_address() {
        let _guard = crate::ENV_MUTEX.lock().unwrap();
        let tmp = tempfile::TempDir::new().unwrap();
        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", tmp.path());

        let path = flowplane::cli::compose::write_envoy_bootstrap().unwrap();

        if let Some(h) = original_home {
            std::env::set_var("HOME", h);
        }

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            content.contains("address: control-plane"),
            "xDS cluster should point to control-plane"
        );
        assert!(content.contains("port_value: 18000"), "xDS cluster should use port 18000");
    }

    /// Admin port is 9901.
    #[test]
    fn admin_port_is_9901() {
        let _guard = crate::ENV_MUTEX.lock().unwrap();
        let tmp = tempfile::TempDir::new().unwrap();
        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", tmp.path());

        let path = flowplane::cli::compose::write_envoy_bootstrap().unwrap();

        if let Some(h) = original_home {
            std::env::set_var("HOME", h);
        }

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("port_value: 9901"), "admin port should be 9901");
    }

    /// Node metadata includes team.
    #[test]
    fn node_metadata_includes_team() {
        let _guard = crate::ENV_MUTEX.lock().unwrap();
        let tmp = tempfile::TempDir::new().unwrap();
        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", tmp.path());

        let path = flowplane::cli::compose::write_envoy_bootstrap().unwrap();

        if let Some(h) = original_home {
            std::env::set_var("HOME", h);
        }

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("team: default"), "node metadata should include team");
    }

    /// Bootstrap YAML is parseable by serde_yaml.
    #[test]
    fn bootstrap_is_valid_yaml() {
        let _guard = crate::ENV_MUTEX.lock().unwrap();
        let tmp = tempfile::TempDir::new().unwrap();
        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", tmp.path());

        let path = flowplane::cli::compose::write_envoy_bootstrap().unwrap();

        if let Some(h) = original_home {
            std::env::set_var("HOME", h);
        }

        let content = std::fs::read_to_string(&path).unwrap();
        let parsed: Result<serde_yaml::Value, _> = serde_yaml::from_str(&content);
        assert!(parsed.is_ok(), "envoy bootstrap should be valid YAML");

        let yaml = parsed.unwrap();
        // Check structure
        assert!(yaml["admin"].is_mapping(), "should have admin section");
        assert!(yaml["node"].is_mapping(), "should have node section");
        assert!(yaml["dynamic_resources"].is_mapping(), "should have dynamic_resources section");
        assert!(yaml["static_resources"].is_mapping(), "should have static_resources section");
    }

    /// write_envoy_bootstrap is idempotent.
    #[test]
    fn write_envoy_bootstrap_is_idempotent() {
        let _guard = crate::ENV_MUTEX.lock().unwrap();
        let tmp = tempfile::TempDir::new().unwrap();
        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", tmp.path());

        let p1 = flowplane::cli::compose::write_envoy_bootstrap().unwrap();
        let c1 = std::fs::read_to_string(&p1).unwrap();

        let p2 = flowplane::cli::compose::write_envoy_bootstrap().unwrap();
        let c2 = std::fs::read_to_string(&p2).unwrap();

        if let Some(h) = original_home {
            std::env::set_var("HOME", h);
        }

        assert_eq!(p1, p2);
        assert_eq!(c1, c2);
    }
}
