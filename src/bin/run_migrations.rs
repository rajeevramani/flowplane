//! Simple migration runner that executes SQL files manually
//! This bypasses SQLx compile-time issues while enabling real database schema

use flowplane::{config::DatabaseConfig, storage::create_pool};
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_max_level(tracing::Level::INFO).init();

    info!("🗄️ Running database migrations manually");

    // Clean up previous test database to ensure fresh state
    let db_path = "./test_flowplane.db";
    if std::path::Path::new(db_path).exists() {
        std::fs::remove_file(db_path)?;
        info!("🗑️ Removed existing test database: {}", db_path);
    }

    // Create database pool
    let db_config = DatabaseConfig {
        url: format!("sqlite://{}", db_path), // Use file-based SQLite for persistence
        max_connections: 5,
        auto_migrate: false, // We'll do it manually
        ..Default::default()
    };

    let pool = create_pool(&db_config).await?;
    info!("✅ Connected to database: {}", db_config.url);

    // Run migrations using the shared dynamic logic
    info!("🚀 Starting migration process...");
    flowplane::storage::run_migrations(&pool).await?;
    
    // Verify tables were created
    info!("🔍 Verifying database schema...");

    let tables = sqlx::query_scalar::<_, String>(
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
    )
    .fetch_all(&pool)
    .await?;

    info!("📊 Created tables: {:?}", tables);

    // Test inserting a sample cluster
    info!("🧪 Testing cluster insertion...");

    let cluster_id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339(); // Convert to string for SQLite compatibility

    sqlx::query(
        r#"
        INSERT INTO clusters (id, name, service_name, configuration, version, created_at, updated_at)
        VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6)
        "#
    )
    .bind(&cluster_id)
    .bind("test_migrated_cluster")
    .bind("test_service")
    .bind(r#"{"type": "EDS", "endpoints": ["127.0.0.1:8080"]}"#)
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await?;

    // Read it back
    let count =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM clusters").fetch_one(&pool).await?;

    info!("✅ Database test complete! Clusters in DB: {}", count);
    info!("🎉 Migration completed successfully!");
    info!("💡 You can now use this database with: DATABASE_URL=sqlite:./test_flowplane.db");

    Ok(())
}
