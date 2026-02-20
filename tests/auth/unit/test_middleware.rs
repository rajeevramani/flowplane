// NOTE: This file requires PostgreSQL - disabled until Phase 4 of PostgreSQL migration
// To run these tests: cargo test --features postgres_tests
#![cfg(feature = "postgres_tests")]

use axum::{
    body::Body,
    http::{Request, StatusCode},
    middleware,
    routing::{get, post},
    Router,
};
use flowplane::auth::{
    auth_service::AuthService,
    middleware::{authenticate, ensure_scopes, ScopeState},
    session::SessionService,
    token_service::TokenService,
};
use flowplane::storage::repository::{AuditLogRepository, SqlxTokenRepository, TokenRepository};
use std::sync::Arc;
use tower::ServiceExt;

#[allow(clippy::duplicate_mod)]
#[path = "../test_schema.rs"]
mod test_schema;
use test_schema::{create_test_pool, TestDatabase};

async fn setup_services() -> (
    TestDatabase,
    TokenService,
    Arc<AuthService>,
    Arc<SessionService>,
    Arc<SqlxTokenRepository>,
    flowplane::storage::DbPool,
) {
    let test_db = create_test_pool().await;
    let pool = test_db.pool.clone();
    let repo = Arc::new(SqlxTokenRepository::new(pool.clone()));
    let audit = Arc::new(AuditLogRepository::new(pool.clone()));

    let token_service = TokenService::new(repo.clone(), audit.clone());
    let auth_service = Arc::new(AuthService::new(repo.clone(), audit.clone()));
    let session_service = Arc::new(SessionService::new(repo.clone(), audit));

    (test_db, token_service, auth_service, session_service, repo, pool)
}

fn secured_router(
    auth_service: Arc<AuthService>,
    session_service: Arc<SessionService>,
    pool: flowplane::storage::DbPool,
    scopes: Vec<&str>,
) -> Router {
    let scope_layer = {
        let required: ScopeState = Arc::new(scopes.into_iter().map(|s| s.to_string()).collect());
        middleware::from_fn_with_state(required, ensure_scopes)
    };

    let auth_state = (auth_service, session_service, pool);

    Router::new()
        .route("/secure", get(|| async { StatusCode::OK }))
        .route_layer(scope_layer)
        .layer(middleware::from_fn_with_state(auth_state, authenticate))
        .with_state(())
}

#[tokio::test]
async fn missing_bearer_returns_unauthorized() {
    let (_db, _, auth_service, session_service, _, pool) = setup_services().await;
    let app = secured_router(auth_service, session_service, pool, vec!["clusters:read"]);

    let response =
        app.oneshot(Request::builder().uri("/secure").body(Body::empty()).unwrap()).await.unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn insufficient_scope_returns_forbidden() {
    let (_db, token_service, auth_service, session_service, _, pool) = setup_services().await;
    let token = token_service
        .create_token(
            flowplane::auth::validation::CreateTokenRequest {
                name: "scope-test".into(),
                description: None,
                expires_at: None,
                scopes: vec!["routes:read".into()],
                created_by: Some("tests".into()),
                user_id: None,
                user_email: None,
            },
            None,
        )
        .await
        .unwrap();

    let app = secured_router(auth_service, session_service, pool, vec!["clusters:write"]);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/secure")
                .header("Authorization", format!("Bearer {}", token.token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn valid_token_allows_request() {
    let (_db, token_service, auth_service, session_service, _, pool) = setup_services().await;
    let token = token_service
        .create_token(
            flowplane::auth::validation::CreateTokenRequest {
                name: "valid-token".into(),
                description: None,
                expires_at: None,
                scopes: vec!["clusters:read".into()],
                created_by: Some("tests".into()),
                user_id: None,
                user_email: None,
            },
            None,
        )
        .await
        .unwrap();

    let app = secured_router(auth_service, session_service, pool, vec!["clusters:read"]);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/secure")
                .header("Authorization", format!("Bearer {}", token.token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

// Cookie and CSRF validation tests

use axum_extra::extract::cookie::Cookie;
use flowplane::auth::{
    models::{NewPersonalAccessToken, TokenStatus},
    session::SESSION_COOKIE_NAME,
};
use flowplane::domain::TokenId;

// Helper to create a session token directly for testing
async fn create_test_session(
    session_service: Arc<SessionService>,
    token_repo: Arc<SqlxTokenRepository>,
    scopes: Vec<&str>,
) -> (String, String) {
    use base64::Engine;
    use rand::RngCore;

    // Generate session token
    let session_id = uuid::Uuid::new_v4().to_string();
    let mut secret_bytes = [0u8; 64];
    rand::rngs::OsRng.fill_bytes(&mut secret_bytes);
    let session_secret = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(secret_bytes);
    let session_token = format!("fp_session_{}.{}", session_id, session_secret);

    // Hash the secret
    let hashed_secret = {
        use argon2::{password_hash::SaltString, Argon2, PasswordHasher};
        let salt = SaltString::generate(&mut rand::rngs::OsRng);
        let argon2 = Argon2::default();
        argon2.hash_password(session_secret.as_bytes(), &salt).unwrap().to_string()
    };

    // Generate CSRF token
    let csrf_token = session_service.generate_csrf_token().unwrap();

    // Create session token in database
    let new_session = NewPersonalAccessToken {
        id: TokenId::from_string(session_id.clone()),
        name: "test-session".to_string(),
        description: Some("Test session token".to_string()),
        hashed_secret,
        status: TokenStatus::Active,
        expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(24)),
        created_by: Some("test".to_string()),
        scopes: scopes.into_iter().map(|s| s.to_string()).collect(),
        is_setup_token: false,
        max_usage_count: None,
        usage_count: 0,
        failed_attempts: 0,
        locked_until: None,
        user_id: None,
        user_email: None,
    };

    token_repo.create_token(new_session).await.unwrap();

    // Store CSRF token
    token_repo.store_csrf_token(&TokenId::from_string(session_id), &csrf_token).await.unwrap();

    (session_token, csrf_token)
}

#[tokio::test]
async fn session_cookie_get_request_succeeds_without_csrf() {
    let (_db, _, auth_service, session_service, token_repo, pool) = setup_services().await;

    let (session_token, _csrf_token) =
        create_test_session(session_service.clone(), token_repo, vec!["clusters:read"]).await;

    let app = secured_router(auth_service, session_service, pool, vec!["clusters:read"]);

    // GET request with session cookie should succeed without CSRF token
    let cookie = Cookie::new(SESSION_COOKIE_NAME, session_token);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/secure")
                .header("Cookie", cookie.to_string())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn session_cookie_post_request_fails_without_csrf() {
    let (_db, _, auth_service, session_service, token_repo, pool) = setup_services().await;

    let (session_token, _csrf_token) =
        create_test_session(session_service.clone(), token_repo, vec!["clusters:write"]).await;

    let app = Router::new()
        .route("/secure", post(|| async { StatusCode::OK }))
        .layer(middleware::from_fn_with_state(
            (auth_service, session_service, pool.clone()),
            authenticate,
        ))
        .with_state(());

    // POST request with session cookie but no CSRF token should fail
    let cookie = Cookie::new(SESSION_COOKIE_NAME, session_token);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/secure")
                .header("Cookie", cookie.to_string())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn session_cookie_post_request_succeeds_with_valid_csrf() {
    let (_db, _, auth_service, session_service, token_repo, pool) = setup_services().await;

    let (session_token, csrf_token) =
        create_test_session(session_service.clone(), token_repo, vec!["clusters:write"]).await;

    let app = Router::new()
        .route("/secure", post(|| async { StatusCode::OK }))
        .layer(middleware::from_fn_with_state(
            (auth_service, session_service, pool.clone()),
            authenticate,
        ))
        .with_state(());

    // POST request with session cookie and valid CSRF token should succeed
    let cookie = Cookie::new(SESSION_COOKIE_NAME, session_token);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/secure")
                .header("Cookie", cookie.to_string())
                .header("X-CSRF-Token", csrf_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn session_cookie_post_request_fails_with_invalid_csrf() {
    let (_db, _, auth_service, session_service, token_repo, pool) = setup_services().await;

    let (session_token, _csrf_token) =
        create_test_session(session_service.clone(), token_repo, vec!["clusters:write"]).await;

    let app = Router::new()
        .route("/secure", post(|| async { StatusCode::OK }))
        .layer(middleware::from_fn_with_state(
            (auth_service, session_service, pool.clone()),
            authenticate,
        ))
        .with_state(());

    // POST request with session cookie and WRONG CSRF token should fail
    let cookie = Cookie::new(SESSION_COOKIE_NAME, session_token);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/secure")
                .header("Cookie", cookie.to_string())
                .header("X-CSRF-Token", "invalid-csrf-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn bearer_session_token_post_request_succeeds_with_valid_csrf() {
    let (_db, _, auth_service, session_service, token_repo, pool) = setup_services().await;

    let (session_token, csrf_token) =
        create_test_session(session_service.clone(), token_repo, vec!["clusters:write"]).await;

    let app = Router::new()
        .route("/secure", post(|| async { StatusCode::OK }))
        .layer(middleware::from_fn_with_state(
            (auth_service, session_service, pool.clone()),
            authenticate,
        ))
        .with_state(());

    // POST request with Bearer session token and valid CSRF token should succeed
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/secure")
                .header("Authorization", format!("Bearer {}", session_token))
                .header("X-CSRF-Token", csrf_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn pat_tokens_bypass_csrf_validation() {
    let (_db, token_service, auth_service, session_service, _, pool) = setup_services().await;

    let token = token_service
        .create_token(
            flowplane::auth::validation::CreateTokenRequest {
                name: "pat-token".into(),
                description: None,
                expires_at: None,
                scopes: vec!["clusters:write".into()],
                created_by: Some("tests".into()),
                user_id: None,
                user_email: None,
            },
            None,
        )
        .await
        .unwrap();

    let app = Router::new()
        .route("/secure", post(|| async { StatusCode::OK }))
        .layer(middleware::from_fn_with_state(
            (auth_service, session_service, pool.clone()),
            authenticate,
        ))
        .with_state(());

    // POST request with PAT token should succeed without CSRF token
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/secure")
                .header("Authorization", format!("Bearer {}", token.token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}
