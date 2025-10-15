use std::sync::Arc;

use flowplane::{
    auth::token_service::TokenService,
    config::SimpleXdsConfig,
    platform_api::materializer::PlatformApiMaterializer,
    storage::{self, repository::AuditLogRepository, DbPool},
    xds::XdsState,
};
use sqlx::sqlite::SqlitePoolOptions;

#[allow(dead_code)]
pub struct MultiApiApp {
    pub state: Arc<XdsState>,
    pub pool: DbPool,
    pub token_service: TokenService,
    pub materializer: PlatformApiMaterializer,
}

pub async fn setup_multi_api_app() -> MultiApiApp {
    // Set BOOTSTRAP_TOKEN for tests that need default gateway resources
    std::env::set_var(
        "BOOTSTRAP_TOKEN",
        "test-bootstrap-token-x8K9mP2nQ5rS7tU9vW1xY3zA4bC6dE8fG0hI2jK4L6m=",
    );

    let pool = SqlitePoolOptions::new()
        .max_connections(10)
        .connect("sqlite::memory:?cache=shared")
        .await
        .expect("create sqlite pool");

    storage::run_migrations(&pool).await.expect("run migrations for tests");

    let state = Arc::new(XdsState::with_database(SimpleXdsConfig::default(), pool.clone()));

    // Ensure default gateway resources exist
    flowplane::openapi::defaults::ensure_default_gateway_resources(&state)
        .await
        .expect("setup default gateway");

    let audit_repo = Arc::new(AuditLogRepository::new(pool.clone()));
    let token_service = TokenService::with_sqlx(pool.clone(), audit_repo);

    let materializer = PlatformApiMaterializer::new(state.clone()).expect("create materializer");

    MultiApiApp { state, pool, token_service, materializer }
}
