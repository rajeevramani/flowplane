//! Database metadata query helpers for tests.
//!
//! Provides PostgreSQL information_schema and pg_indexes based metadata queries
//! to enable schema validation in integration tests.

use flowplane::storage::DbPool;
use sqlx::Row;

/// Column metadata information
#[derive(Debug, Clone)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub is_nullable: bool,
    pub default_value: Option<String>,
}

/// Index metadata information
#[derive(Debug, Clone)]
pub struct IndexInfo {
    pub name: String,
}

/// Get column information for a table using PostgreSQL information_schema.
pub async fn get_table_columns(
    pool: &DbPool,
    table_name: &str,
) -> Result<Vec<ColumnInfo>, sqlx::Error> {
    let rows = sqlx::query(
        r#"
        SELECT
            column_name,
            data_type,
            is_nullable,
            column_default
        FROM information_schema.columns
        WHERE table_schema = 'public'
          AND table_name = $1
        ORDER BY ordinal_position
        "#,
    )
    .bind(table_name)
    .fetch_all(pool)
    .await?;

    let columns = rows
        .iter()
        .map(|row| {
            let name: String = row.get("column_name");
            let data_type: String = row.get("data_type");
            let is_nullable_str: String = row.get("is_nullable");
            let default_value: Option<String> = row.get("column_default");

            ColumnInfo { name, data_type, is_nullable: is_nullable_str == "YES", default_value }
        })
        .collect();

    Ok(columns)
}

/// Get index information for a table using PostgreSQL pg_indexes.
pub async fn get_table_indexes(
    pool: &DbPool,
    table_name: &str,
) -> Result<Vec<IndexInfo>, sqlx::Error> {
    let rows = sqlx::query(
        r#"
        SELECT indexname as name
        FROM pg_indexes
        WHERE schemaname = 'public'
          AND tablename = $1
        "#,
    )
    .bind(table_name)
    .fetch_all(pool)
    .await?;

    let indexes = rows
        .iter()
        .map(|row| {
            let name: String = row.get("name");
            IndexInfo { name }
        })
        .collect();

    Ok(indexes)
}

/// Check if a table has a specific column.
pub async fn table_has_column(
    pool: &DbPool,
    table_name: &str,
    column_name: &str,
) -> Result<bool, sqlx::Error> {
    let columns = get_table_columns(pool, table_name).await?;
    Ok(columns.iter().any(|c| c.name == column_name))
}

/// Check if a table has a specific index.
pub async fn table_has_index(
    pool: &DbPool,
    table_name: &str,
    index_name: &str,
) -> Result<bool, sqlx::Error> {
    let indexes = get_table_indexes(pool, table_name).await?;
    Ok(indexes.iter().any(|i| i.name == index_name))
}

/// Get all column names for a table (convenience method).
pub async fn get_column_names(pool: &DbPool, table_name: &str) -> Result<Vec<String>, sqlx::Error> {
    let columns = get_table_columns(pool, table_name).await?;
    Ok(columns.into_iter().map(|c| c.name).collect())
}

/// Get all index names for a table (convenience method).
pub async fn get_index_names(pool: &DbPool, table_name: &str) -> Result<Vec<String>, sqlx::Error> {
    let indexes = get_table_indexes(pool, table_name).await?;
    Ok(indexes.into_iter().map(|i| i.name).collect())
}
