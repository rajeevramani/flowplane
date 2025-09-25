//! Simple migration runner that executes SQL files manually
//! This bypasses SQLx compile-time issues while enabling real database schema

use flowplane::{config::DatabaseConfig, storage::create_pool};
use std::fs;
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    info!("🗄️ Running database migrations manually");

    // Create database pool
    let db_config = DatabaseConfig {
        url: "sqlite://./test_flowplane.db".to_string(), // Use file-based SQLite for persistence
        max_connections: 5,
        auto_migrate: false, // We'll do it manually
        ..Default::default()
    };

    let pool = create_pool(&db_config).await?;
    info!("✅ Connected to database: {}", db_config.url);

    // Read and execute migration files in order
    let migration_files = [
        "migrations/20241201000001_create_clusters_table.sql",
        "migrations/20241201000002_create_routes_table.sql",
        "migrations/20241201000003_create_listeners_table.sql",
        "migrations/20241201000004_create_configuration_versions_table.sql",
        "migrations/20241201000005_create_audit_log_table.sql",
    ];

    for migration_file in &migration_files {
        info!("📜 Executing migration: {}", migration_file);

        let sql_content = fs::read_to_string(migration_file)
            .map_err(|e| format!("Failed to read {}: {}", migration_file, e))?;

        // Execute the SQL
        sqlx::raw_sql(&sql_content)
            .execute(&pool)
            .await
            .map_err(|e| format!("Failed to execute {}: {}", migration_file, e))?;

        info!("✅ Applied: {}", migration_file);
    }

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
    let count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM clusters")
        .fetch_one(&pool)
        .await?;

    info!("✅ Database test complete! Clusters in DB: {}", count);
    info!("🎉 Migration completed successfully!");
    info!("💡 You can now use this database with: DATABASE_URL=sqlite:./test_flowplane.db");

    Ok(())
}
