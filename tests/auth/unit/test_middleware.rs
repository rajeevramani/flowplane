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
use sqlx::sqlite::SqlitePoolOptions;
use std::sync::Arc;
use tower::ServiceExt;

async fn setup_pool() -> flowplane::storage::DbPool {
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect("sqlite::memory:?cache=shared")
        .await
        .expect("create sqlite pool");

    sqlx::query(
        r#"
        CREATE TABLE personal_access_tokens (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            token_hash TEXT NOT NULL,
            description TEXT,
            status TEXT NOT NULL,
            expires_at DATETIME,
            last_used_at DATETIME,
            created_by TEXT,
            created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
            updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,

            is_setup_token BOOLEAN NOT NULL DEFAULT FALSE,
            max_usage_count INTEGER,
            usage_count INTEGER NOT NULL DEFAULT 0,
            failed_attempts INTEGER NOT NULL DEFAULT 0,
            locked_until DATETIME,
            csrf_token TEXT
        );
        "#,
    )
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        r#"
        CREATE TABLE token_scopes (
            id TEXT PRIMARY KEY,
            token_id TEXT NOT NULL,
            scope TEXT NOT NULL,
            created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY (token_id) REFERENCES personal_access_tokens(id) ON DELETE CASCADE
        );
        "#,
    )
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        r#"
        CREATE TABLE audit_log (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            resource_type TEXT NOT NULL,
            resource_id TEXT,
            resource_name TEXT,
            action TEXT NOT NULL,
            old_configuration TEXT,
            new_configuration TEXT,
            user_id TEXT,
            client_ip TEXT,
            user_agent TEXT,
            created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
        );
        "#,
    )
    .execute(&pool)
    .await
    .unwrap();

    pool
}

async fn setup_services(
) -> (TokenService, Arc<AuthService>, Arc<SessionService>, Arc<SqlxTokenRepository>) {
    let pool = setup_pool().await;
    let repo = Arc::new(SqlxTokenRepository::new(pool.clone()));
    let audit = Arc::new(AuditLogRepository::new(pool));

    let token_service = TokenService::new(repo.clone(), audit.clone());
    let auth_service = Arc::new(AuthService::new(repo.clone(), audit.clone()));
    let session_service = Arc::new(SessionService::new(repo.clone(), audit));

    (token_service, auth_service, session_service, repo)
}

fn secured_router(
    auth_service: Arc<AuthService>,
    session_service: Arc<SessionService>,
    scopes: Vec<&str>,
) -> Router {
    let scope_layer = {
        let required: ScopeState = Arc::new(scopes.into_iter().map(|s| s.to_string()).collect());
        middleware::from_fn_with_state(required, ensure_scopes)
    };

    let auth_state = (auth_service, session_service);

    Router::new()
        .route("/secure", get(|| async { StatusCode::OK }))
        .route_layer(scope_layer)
        .layer(middleware::from_fn_with_state(auth_state, authenticate))
        .with_state(())
}

#[tokio::test]
async fn missing_bearer_returns_unauthorized() {
    let (_, auth_service, session_service, _) = setup_services().await;
    let app = secured_router(auth_service, session_service, vec!["clusters:read"]);

    let response =
        app.oneshot(Request::builder().uri("/secure").body(Body::empty()).unwrap()).await.unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn insufficient_scope_returns_forbidden() {
    let (token_service, auth_service, session_service, _) = setup_services().await;
    let token = token_service
        .create_token(flowplane::auth::validation::CreateTokenRequest {
            name: "scope-test".into(),
            description: None,
            expires_at: None,
            scopes: vec!["routes:read".into()],
            created_by: Some("tests".into()),
        })
        .await
        .unwrap();

    let app = secured_router(auth_service, session_service, vec!["clusters:write"]);

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
    let (token_service, auth_service, session_service, _) = setup_services().await;
    let token = token_service
        .create_token(flowplane::auth::validation::CreateTokenRequest {
            name: "valid-token".into(),
            description: None,
            expires_at: None,
            scopes: vec!["clusters:read".into()],
            created_by: Some("tests".into()),
        })
        .await
        .unwrap();

    let app = secured_router(auth_service, session_service, vec!["clusters:read"]);

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
    let (_, auth_service, session_service, token_repo) = setup_services().await;

    let (session_token, _csrf_token) =
        create_test_session(session_service.clone(), token_repo, vec!["clusters:read"]).await;

    let app = secured_router(auth_service, session_service, vec!["clusters:read"]);

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
    let (_, auth_service, session_service, token_repo) = setup_services().await;

    let (session_token, _csrf_token) =
        create_test_session(session_service.clone(), token_repo, vec!["clusters:write"]).await;

    let app = Router::new()
        .route("/secure", post(|| async { StatusCode::OK }))
        .layer(middleware::from_fn_with_state((auth_service, session_service), authenticate))
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
    let (_, auth_service, session_service, token_repo) = setup_services().await;

    let (session_token, csrf_token) =
        create_test_session(session_service.clone(), token_repo, vec!["clusters:write"]).await;

    let app = Router::new()
        .route("/secure", post(|| async { StatusCode::OK }))
        .layer(middleware::from_fn_with_state((auth_service, session_service), authenticate))
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
    let (_, auth_service, session_service, token_repo) = setup_services().await;

    let (session_token, _csrf_token) =
        create_test_session(session_service.clone(), token_repo, vec!["clusters:write"]).await;

    let app = Router::new()
        .route("/secure", post(|| async { StatusCode::OK }))
        .layer(middleware::from_fn_with_state((auth_service, session_service), authenticate))
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
    let (_, auth_service, session_service, token_repo) = setup_services().await;

    let (session_token, csrf_token) =
        create_test_session(session_service.clone(), token_repo, vec!["clusters:write"]).await;

    let app = Router::new()
        .route("/secure", post(|| async { StatusCode::OK }))
        .layer(middleware::from_fn_with_state((auth_service, session_service), authenticate))
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
    let (token_service, auth_service, session_service, _) = setup_services().await;

    let token = token_service
        .create_token(flowplane::auth::validation::CreateTokenRequest {
            name: "pat-token".into(),
            description: None,
            expires_at: None,
            scopes: vec!["clusters:write".into()],
            created_by: Some("tests".into()),
        })
        .await
        .unwrap();

    let app = Router::new()
        .route("/secure", post(|| async { StatusCode::OK }))
        .layer(middleware::from_fn_with_state((auth_service, session_service), authenticate))
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
