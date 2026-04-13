// Adversarial integration tests for the xds_nack_events schema migration
// (fp-hsk.2). Tests are gated behind the postgres_tests feature and exercise
// real PostgreSQL via testcontainers.
//
// These tests are written from the migration SPEC (bead fp-hsk.2 + the two
// migration .sql files) WITHOUT reading the Rust repository implementation
// (src/storage/repositories/nack_event.rs) or its callers. They assert on
// observable database behavior: what inserts the constraints accept and
// reject, and whether the dedup index behaves as specified.
#![cfg(feature = "postgres_tests")]

mod common;

use common::test_db::TestDatabase;
use flowplane::storage::repositories::{CreateNackEventRequest, NackEventRepository, NackSource};

// ---------- Raw-SQL helpers ----------

/// Build an INSERT that provides every NOT NULL column we know about from the
/// original table definition, plus the columns added in the migration when
/// given. The caller supplies the `source`, `nonce`, `version_rejected`, and
/// `dedup_hash` values via bind parameters so the test can exercise edge
/// cases (NULL, duplicates, invalid enum values).
async fn raw_insert(
    pool: &flowplane::storage::DbPool,
    id: &str,
    source: Option<&str>,
    nonce: Option<&str>,
    version_rejected: Option<&str>,
    dedup_hash: Option<&str>,
) -> Result<u64, sqlx::Error> {
    // When `source` is None, omit the column to exercise the DEFAULT 'stream'.
    if let Some(source_value) = source {
        let result = sqlx::query(
            "INSERT INTO xds_nack_events
                (id, team, dataplane_name, type_url, version_rejected, nonce,
                 error_code, error_message, source, dedup_hash)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(id)
        .bind("00000000-0000-0000-0000-000000000001")
        .bind("dp-1")
        .bind("type.googleapis.com/envoy.config.cluster.v3.Cluster")
        .bind(version_rejected)
        .bind(nonce)
        .bind(13i64)
        .bind("boom")
        .bind(source_value)
        .bind(dedup_hash)
        .execute(pool)
        .await?;
        Ok(result.rows_affected())
    } else {
        let result = sqlx::query(
            "INSERT INTO xds_nack_events
                (id, team, dataplane_name, type_url, version_rejected, nonce,
                 error_code, error_message, dedup_hash)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(id)
        .bind("00000000-0000-0000-0000-000000000001")
        .bind("dp-1")
        .bind("type.googleapis.com/envoy.config.cluster.v3.Cluster")
        .bind(version_rejected)
        .bind(nonce)
        .bind(13i64)
        .bind("boom")
        .bind(dedup_hash)
        .execute(pool)
        .await?;
        Ok(result.rows_affected())
    }
}

// ---------- Schema / constraint tests (raw SQL) ----------

#[tokio::test]
async fn warming_report_with_null_nonce_and_version_is_accepted() {
    let db = TestDatabase::new("nack_warming_null").await;
    let rows =
        raw_insert(&db.pool, "row-warming-1", Some("warming_report"), None, None, Some("hash-abc"))
            .await
            .expect("warming_report with null nonce/version must succeed");
    assert_eq!(rows, 1);

    // Roundtrip: verify the row is readable and the columns are what we wrote.
    let (source, nonce, version_rejected, dedup_hash): (
        String,
        Option<String>,
        Option<String>,
        Option<String>,
    ) = sqlx::query_as(
        "SELECT source, nonce, version_rejected, dedup_hash
         FROM xds_nack_events WHERE id = $1",
    )
    .bind("row-warming-1")
    .fetch_one(&db.pool)
    .await
    .expect("row should be readable");
    assert_eq!(source, "warming_report");
    assert_eq!(nonce, None);
    assert_eq!(version_rejected, None);
    assert_eq!(dedup_hash.as_deref(), Some("hash-abc"));
}

#[tokio::test]
async fn invalid_source_value_is_rejected_by_check_constraint() {
    let db = TestDatabase::new("nack_invalid_source").await;
    let result = raw_insert(
        &db.pool,
        "row-bad-source",
        Some("not_a_real_source"),
        Some("nonce"),
        Some("v1"),
        None,
    )
    .await;
    assert!(result.is_err(), "CHECK constraint should reject source='not_a_real_source'");
    // PostgreSQL unique/check violations come back as database errors.
    // We assert the error mentions the check constraint to avoid false
    // positives from other errors (e.g. connection failures).
    let err = format!("{:?}", result.unwrap_err());
    assert!(
        err.contains("check") || err.contains("source") || err.contains("violates"),
        "error should mention constraint violation, got: {err}"
    );
}

#[tokio::test]
async fn default_source_backfill_is_stream_when_column_omitted() {
    let db = TestDatabase::new("nack_default_source").await;
    let rows = raw_insert(
        &db.pool,
        "row-default",
        None, // omit source column -> must default to 'stream'
        Some("nonce-x"),
        Some("v-1"),
        None,
    )
    .await
    .expect("insert without source should succeed via DEFAULT");
    assert_eq!(rows, 1);

    let source: String = sqlx::query_scalar("SELECT source FROM xds_nack_events WHERE id = $1")
        .bind("row-default")
        .fetch_one(&db.pool)
        .await
        .expect("row should be readable");
    assert_eq!(
        source, "stream",
        "DEFAULT 'stream' must backfill rows that omit source (migration spec)"
    );
}

#[tokio::test]
async fn stream_source_with_null_nonce_is_accepted_post_migration() {
    // Pre-migration nonce was NOT NULL. The migration drops NOT NULL for
    // both sources, so a stream-source row with NULL nonce must now insert.
    let db = TestDatabase::new("nack_stream_null_nonce").await;
    let rows = raw_insert(&db.pool, "row-stream-null-nonce", Some("stream"), None, None, None)
        .await
        .expect("stream source with null nonce/version must succeed");
    assert_eq!(rows, 1);
}

#[tokio::test]
async fn duplicate_non_null_dedup_hash_is_rejected() {
    let db = TestDatabase::new("nack_dedup_dup").await;

    raw_insert(&db.pool, "row-dedup-1", Some("warming_report"), None, None, Some("same-hash"))
        .await
        .expect("first warming_report must insert");

    let second =
        raw_insert(&db.pool, "row-dedup-2", Some("warming_report"), None, None, Some("same-hash"))
            .await;

    assert!(
        second.is_err(),
        "second row with same non-null dedup_hash must be rejected by unique partial index"
    );
    let err = format!("{:?}", second.unwrap_err());
    assert!(
        err.contains("duplicate") || err.contains("unique") || err.contains("violates"),
        "expected unique-violation error, got: {err}"
    );
}

#[tokio::test]
async fn multiple_rows_with_null_dedup_hash_are_allowed() {
    let db = TestDatabase::new("nack_null_hashes").await;

    for i in 0..3 {
        raw_insert(
            &db.pool,
            &format!("row-null-hash-{i}"),
            Some("stream"),
            Some("n"),
            Some("v"),
            None,
        )
        .await
        .unwrap_or_else(|e| panic!("row {i} with NULL dedup_hash must insert: {e:?}"));
    }

    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM xds_nack_events WHERE dedup_hash IS NULL")
            .fetch_one(&db.pool)
            .await
            .expect("count query should succeed");
    assert_eq!(count, 3, "partial unique index must NOT fire on NULL values");
}

#[tokio::test]
async fn dedup_hash_unique_index_exists_with_expected_name() {
    let db = TestDatabase::new("nack_index_name").await;

    // Per migration file 20260413000001: index is named
    // xds_nack_events_dedup_hash_idx. The bead description wrote
    // nack_events_dedup_hash_idx but the migration is the source of truth.
    let index_name: Option<String> = sqlx::query_scalar(
        "SELECT indexname FROM pg_indexes
         WHERE tablename = 'xds_nack_events'
           AND indexname = 'xds_nack_events_dedup_hash_idx'",
    )
    .fetch_optional(&db.pool)
    .await
    .expect("pg_indexes query should succeed");
    assert!(
        index_name.is_some(),
        "expected unique partial index 'xds_nack_events_dedup_hash_idx' on xds_nack_events"
    );

    // Also verify it is UNIQUE and partial (has a WHERE clause).
    let (is_unique, has_where): (bool, bool) = sqlx::query_as(
        "SELECT i.indisunique,
                pg_get_expr(i.indpred, i.indrelid) IS NOT NULL
         FROM pg_index i
         JOIN pg_class c ON c.oid = i.indexrelid
         WHERE c.relname = 'xds_nack_events_dedup_hash_idx'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("pg_index query should succeed");
    assert!(is_unique, "dedup_hash index must be UNIQUE");
    assert!(has_where, "dedup_hash index must be partial (WHERE clause)");
}

#[tokio::test]
async fn source_column_is_not_null_with_stream_default() {
    let db = TestDatabase::new("nack_source_notnull").await;

    let (is_nullable, default_expr): (String, Option<String>) = sqlx::query_as(
        "SELECT is_nullable, column_default
         FROM information_schema.columns
         WHERE table_name = 'xds_nack_events' AND column_name = 'source'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("information_schema query should succeed");
    assert_eq!(is_nullable, "NO", "source column must be NOT NULL");
    let default_expr = default_expr.expect("source column must have a DEFAULT");
    assert!(
        default_expr.contains("stream"),
        "source DEFAULT must be 'stream', got: {default_expr}"
    );
}

#[tokio::test]
async fn nonce_and_version_rejected_columns_are_nullable() {
    let db = TestDatabase::new("nack_columns_nullable").await;

    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT column_name, is_nullable
         FROM information_schema.columns
         WHERE table_name = 'xds_nack_events'
           AND column_name IN ('nonce', 'version_rejected', 'dedup_hash')
         ORDER BY column_name",
    )
    .fetch_all(&db.pool)
    .await
    .expect("information_schema query should succeed");

    for (col, nullable) in &rows {
        assert_eq!(nullable, "YES", "column {col} must be nullable after migration");
    }
    assert_eq!(rows.len(), 3, "expected nonce, version_rejected, dedup_hash");
}

// ---------- Rust contract tests (via NackEventRepository) ----------
//
// These exercise the Rust API surface listed in the bead: `source`,
// `dedup_hash`, nullable `nonce`/`version_rejected`. Using the repository
// (not raw SQL) here catches mismatches between the struct field set and
// the SQL insert.

#[tokio::test]
async fn repository_insert_warming_report_with_nones_succeeds() {
    let db = TestDatabase::new("nack_repo_warming").await;
    let repo = NackEventRepository::new(db.pool.clone());

    let req = CreateNackEventRequest {
        team: "00000000-0000-0000-0000-000000000001".to_string(),
        dataplane_name: "dp-warming".to_string(),
        type_url: "type.googleapis.com/envoy.config.listener.v3.Listener".to_string(),
        version_rejected: None,
        nonce: None,
        error_code: 3,
        error_message: "listener failed to warm".to_string(),
        node_id: Some("node-1".to_string()),
        resource_names: Some("[\"listener_0\"]".to_string()),
        source: NackSource::WarmingReport,
        dedup_hash: Some("unique-warming-hash".to_string()),
    };

    repo.insert(req).await.expect("repository must accept warming_report with None nonce/version");

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM xds_nack_events
         WHERE source = 'warming_report' AND dedup_hash = 'unique-warming-hash'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn repository_insert_stream_with_empty_string_nonce_succeeds() {
    // Empty string is a valid, non-null value. Historical stream NACK code
    // may pass "" — must still work.
    let db = TestDatabase::new("nack_repo_empty_nonce").await;
    let repo = NackEventRepository::new(db.pool.clone());

    let req = CreateNackEventRequest {
        team: "00000000-0000-0000-0000-000000000001".to_string(),
        dataplane_name: "dp-stream".to_string(),
        type_url: "type.googleapis.com/envoy.config.cluster.v3.Cluster".to_string(),
        version_rejected: Some(String::new()),
        nonce: Some(String::new()),
        error_code: 13,
        error_message: "rejected".to_string(),
        node_id: None,
        resource_names: None,
        source: NackSource::Stream,
        dedup_hash: None,
    };

    repo.insert(req).await.expect("empty-string nonce must be accepted");

    let (nonce, version_rejected): (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT nonce, version_rejected FROM xds_nack_events WHERE dataplane_name = 'dp-stream'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(nonce.as_deref(), Some(""));
    assert_eq!(version_rejected.as_deref(), Some(""));
}

#[tokio::test]
async fn repository_insert_duplicate_dedup_hash_surfaces_error() {
    let db = TestDatabase::new("nack_repo_dedup").await;
    let repo = NackEventRepository::new(db.pool.clone());

    let build = |dp: &str| CreateNackEventRequest {
        team: "00000000-0000-0000-0000-000000000001".to_string(),
        dataplane_name: dp.to_string(),
        type_url: "type.googleapis.com/envoy.config.cluster.v3.Cluster".to_string(),
        version_rejected: None,
        nonce: None,
        error_code: 13,
        error_message: "boom".to_string(),
        node_id: None,
        resource_names: None,
        source: NackSource::WarmingReport,
        dedup_hash: Some("dup-hash-xyz".to_string()),
    };

    repo.insert(build("dp-a")).await.expect("first insert must succeed");
    let second = repo.insert(build("dp-b")).await;
    assert!(
        second.is_err(),
        "second insert with same dedup_hash must surface an error to the caller"
    );
}
