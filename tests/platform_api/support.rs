use std::sync::Arc;

use axum::{
    body::to_bytes,
    body::Body,
    http::{Method, Request},
    Router,
};
use flowplane::{
    auth::{
        team::CreateTeamRequest,
        token_service::{TokenSecretResponse, TokenService},
        validation::CreateTokenRequest,
    },
    config::SimpleXdsConfig,
    storage::{
        self,
        repositories::team::{SqlxTeamRepository, TeamRepository},
        repository::AuditLogRepository,
        DbPool,
    },
    xds::XdsState,
};
use hyper::Response;
use serde::de::DeserializeOwned;
use serde_json::Value;
use sqlx::sqlite::SqlitePoolOptions;
use tower::ServiceExt;

pub struct PlatformApiApp {
    state: Arc<XdsState>,
    pub pool: DbPool,
    token_service: TokenService,
}

impl PlatformApiApp {
    pub fn router(&self) -> Router {
        flowplane::api::routes::build_router(self.state.clone())
    }

    pub async fn issue_token(&self, name: &str, scopes: &[&str]) -> TokenSecretResponse {
        self.token_service
            .create_token(CreateTokenRequest::without_user(
                name.to_string(),
                None,
                None,
                scopes.iter().map(|scope| scope.to_string()).collect(),
                Some("platform-api-tests".into()),
            ))
            .await
            .expect("create token")
    }
}

pub async fn setup_platform_api_app() -> PlatformApiApp {
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect("sqlite::memory:?cache=shared")
        .await
        .expect("create sqlite pool");

    storage::run_migrations(&pool).await.expect("run migrations for tests");

    // Create common teams used across tests
    let team_names =
        vec!["billing", "payments", "platform-team", "team-a", "team-b", "test", "test-team"];
    for team_name in team_names {
        create_team(&pool, team_name).await;
    }

    let state = Arc::new(XdsState::with_database(SimpleXdsConfig::default(), pool.clone()));
    let audit_repo = Arc::new(AuditLogRepository::new(pool.clone()));
    let token_service = TokenService::with_sqlx(pool.clone(), audit_repo);

    PlatformApiApp { state, pool, token_service }
}

pub async fn send_request(
    app: &PlatformApiApp,
    method: Method,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> Response<Body> {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(token) = token {
        builder = builder.header("Authorization", format!("Bearer {}", token));
    }

    let request = if let Some(json) = body {
        let bytes = serde_json::to_vec(&json).expect("serialize body");
        builder
            .header("content-type", "application/json")
            .body(Body::from(bytes))
            .expect("build request")
    } else {
        builder.body(Body::empty()).expect("build request")
    };

    app.router().oneshot(request).await.expect("request")
}

pub async fn read_json<T: DeserializeOwned>(response: Response<Body>) -> T {
    let bytes =
        to_bytes(response.into_body(), usize::MAX).await.expect("read response body as bytes");
    serde_json::from_slice(&bytes).expect("parse json response")
}

pub async fn create_team(pool: &DbPool, name: &str) {
    let team_repo = SqlxTeamRepository::new(pool.clone());
    let _ = team_repo
        .create_team(CreateTeamRequest {
            name: name.to_string(),
            display_name: format!("Test Team {}", name),
            description: Some("Team for platform API tests".to_string()),
            owner_user_id: None,
            settings: None,
        })
        .await;
}
