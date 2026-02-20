//! Manual migration runner for PostgreSQL
//!
//! Connects to a PostgreSQL database and runs all pending migrations.
//! Usage: cargo run --bin run_migrations
//!
//! Set FLOWPLANE_DATABASE_URL to specify the target database.
//! Defaults to postgresql://localhost:5432/flowplane

use flowplane::{config::DatabaseConfig, storage::create_pool};
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_max_level(tracing::Level::INFO).init();

    info!("Running database migrations");

    let db_config = DatabaseConfig {
        url: std::env::var("FLOWPLANE_DATABASE_URL")
            .unwrap_or_else(|_| "postgresql://localhost:5432/flowplane".to_string()),
        max_connections: 5,
        auto_migrate: false, // We'll do it manually below
        ..Default::default()
    };

    let pool = create_pool(&db_config).await?;
    info!("Connected to database");

    info!("Starting migration process...");
    flowplane::storage::run_migrations(&pool).await?;

    // Verify tables were created
    let tables = sqlx::query_scalar::<_, String>(
        "SELECT tablename FROM pg_tables WHERE schemaname = 'public' ORDER BY tablename",
    )
    .fetch_all(&pool)
    .await?;

    info!("Tables in database: {:?}", tables);

    let count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM _flowplane_migrations")
        .fetch_one(&pool)
        .await?;

    info!("Migrations applied: {}", count);
    info!("Migration completed successfully");

    Ok(())
}
