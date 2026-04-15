//! NACK event repository for persisting xDS NACK events
//!
//! This module provides insert and query operations for xDS NACK events,
//! representing configuration rejections from Envoy dataplanes.

use crate::errors::{FlowplaneError, Result};
use crate::storage::DbPool;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::fmt;
use tracing::instrument;

/// Origin of a NACK event.
///
/// `Stream` corresponds to a classic xDS stream-level NACK (Envoy rejected a
/// DiscoveryResponse inline with `error_detail`). `WarmingReport` corresponds
/// to a warming failure surfaced by the dataplane-side agent from Envoy's
/// `/config_dump` `error_state`. The two sources have different required
/// fields — warming reports have no nonce or version — which is why nonce and
/// version_rejected are nullable at the DB layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NackSource {
    Stream,
    WarmingReport,
}

impl NackSource {
    /// String representation persisted to the `source` column. MUST match the
    /// values accepted by the CHECK constraint in the `xds_nack_events` table.
    pub fn as_str(self) -> &'static str {
        match self {
            NackSource::Stream => "stream",
            NackSource::WarmingReport => "warming_report",
        }
    }

    /// Parses a value read from the database. Unknown values produce an error
    /// rather than silently mapping to a default — the CHECK constraint should
    /// make this unreachable in practice, but we treat it as a hard failure to
    /// surface schema drift.
    pub fn from_db_str(value: &str) -> Result<Self> {
        match value {
            "stream" => Ok(NackSource::Stream),
            "warming_report" => Ok(NackSource::WarmingReport),
            other => Err(FlowplaneError::internal(format!(
                "unknown nack_events source value: {}",
                other
            ))),
        }
    }
}

impl fmt::Display for NackSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Internal database row structure for NACK events.
///
/// `team` is the team **id** (FK to `teams.id`) after migration
/// `20260415000001_unify_xds_nack_events_team_to_team_id.sql`. The column was
/// previously a TEXT team name with no FK; the rename was made to fix fp-4g4,
/// where the insert path wrote names while the query path read ids and
/// therefore matched zero rows.
#[derive(Debug, Clone, FromRow)]
struct NackEventRow {
    pub id: String,
    pub team: String,
    pub dataplane_name: String,
    pub type_url: String,
    pub version_rejected: Option<String>,
    pub nonce: Option<String>,
    pub error_code: i64,
    pub error_message: String,
    pub node_id: Option<String>,
    pub resource_names: Option<String>,
    pub source: String,
    pub dedup_hash: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// NACK event data returned from the repository.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NackEventData {
    pub id: String,
    pub team: String,
    pub dataplane_name: String,
    pub type_url: String,
    pub version_rejected: Option<String>,
    pub nonce: Option<String>,
    pub error_code: i64,
    pub error_message: String,
    pub node_id: Option<String>,
    pub resource_names: Option<String>,
    pub source: NackSource,
    pub dedup_hash: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl TryFrom<NackEventRow> for NackEventData {
    type Error = FlowplaneError;

    fn try_from(row: NackEventRow) -> Result<Self> {
        Ok(Self {
            id: row.id,
            team: row.team,
            dataplane_name: row.dataplane_name,
            type_url: row.type_url,
            version_rejected: row.version_rejected,
            nonce: row.nonce,
            error_code: row.error_code,
            error_message: row.error_message,
            node_id: row.node_id,
            resource_names: row.resource_names,
            source: NackSource::from_db_str(&row.source)?,
            dedup_hash: row.dedup_hash,
            created_at: row.created_at,
        })
    }
}

/// Request to create a new NACK event.
///
/// `nonce` and `version_rejected` are optional because warming-report sources
/// do not carry that information. For `source = NackSource::Stream` the caller
/// should populate them from the rejected DiscoveryRequest.
///
/// `team` is the team **id** (FK to `teams.id`), NOT the team name. Callers
/// that hold a team name (e.g. SPIFFE-extracted) MUST resolve it via
/// `XdsState::resolve_team_name` (or equivalent SQL lookup on `teams`) before
/// constructing this request — otherwise the insert will FK-fail. See fp-4g4.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateNackEventRequest {
    pub team: String,
    pub dataplane_name: String,
    pub type_url: String,
    pub version_rejected: Option<String>,
    pub nonce: Option<String>,
    pub error_code: i64,
    pub error_message: String,
    pub node_id: Option<String>,
    pub resource_names: Option<String>,
    pub source: NackSource,
    pub dedup_hash: Option<String>,
}

/// Repository for xDS NACK event persistence.
#[derive(Debug, Clone)]
pub struct NackEventRepository {
    pool: DbPool,
}

impl NackEventRepository {
    /// Creates a new NACK event repository with the given database pool.
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Inserts a new NACK event into the database.
    #[instrument(skip(self, request), fields(team = %request.team, dataplane = %request.dataplane_name, type_url = %request.type_url), name = "db_insert_nack_event")]
    pub async fn insert(&self, request: CreateNackEventRequest) -> Result<NackEventData> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();

        sqlx::query(
            "INSERT INTO xds_nack_events (id, team, dataplane_name, type_url, version_rejected, nonce, error_code, error_message, node_id, resource_names, source, dedup_hash, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)"
        )
        .bind(&id)
        .bind(&request.team)
        .bind(&request.dataplane_name)
        .bind(&request.type_url)
        .bind(&request.version_rejected)
        .bind(&request.nonce)
        .bind(request.error_code)
        .bind(&request.error_message)
        .bind(&request.node_id)
        .bind(&request.resource_names)
        .bind(request.source.as_str())
        .bind(&request.dedup_hash)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, team = %request.team, dataplane = %request.dataplane_name, "Failed to insert NACK event");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to insert NACK event for dataplane '{}'", request.dataplane_name),
            }
        })?;

        tracing::info!(nack_id = %id, team = %request.team, dataplane = %request.dataplane_name, type_url = %request.type_url, "Inserted NACK event");

        Ok(NackEventData {
            id,
            team: request.team,
            dataplane_name: request.dataplane_name,
            type_url: request.type_url,
            version_rejected: request.version_rejected,
            nonce: request.nonce,
            error_code: request.error_code,
            error_message: request.error_message,
            node_id: request.node_id,
            resource_names: request.resource_names,
            source: request.source,
            dedup_hash: request.dedup_hash,
            created_at: now,
        })
    }

    /// Lists NACK events for a specific team, ordered by most recent first.
    #[instrument(skip(self), fields(team = %team, limit = ?limit, offset = ?offset), name = "db_list_nack_events_by_team")]
    pub async fn list_by_team(
        &self,
        team: &str,
        limit: Option<i32>,
        offset: Option<i32>,
    ) -> Result<Vec<NackEventData>> {
        let limit = limit.unwrap_or(100).min(1000);
        let offset = offset.unwrap_or(0);

        let rows = sqlx::query_as::<sqlx::Postgres, NackEventRow>(
            "SELECT id, team, dataplane_name, type_url, version_rejected, nonce, error_code, error_message, node_id, resource_names, source, dedup_hash, created_at \
             FROM xds_nack_events WHERE team = $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3"
        )
        .bind(team)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, team = %team, "Failed to list NACK events by team");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list NACK events for team '{}'", team),
            }
        })?;

        rows.into_iter().map(NackEventData::try_from).collect()
    }

    /// Lists NACK events for a specific dataplane within a team.
    #[instrument(skip(self), fields(team = %team, dataplane = %dataplane_name, since = ?since, limit = ?limit), name = "db_list_nack_events_by_dataplane")]
    pub async fn list_by_dataplane(
        &self,
        team: &str,
        dataplane_name: &str,
        since: Option<DateTime<Utc>>,
        limit: Option<i32>,
    ) -> Result<Vec<NackEventData>> {
        let limit = limit.unwrap_or(100).min(1000);

        let rows = match since {
            Some(since_time) => {
                sqlx::query_as::<sqlx::Postgres, NackEventRow>(
                    "SELECT id, team, dataplane_name, type_url, version_rejected, nonce, error_code, error_message, node_id, resource_names, source, dedup_hash, created_at \
                     FROM xds_nack_events WHERE team = $1 AND dataplane_name = $2 AND created_at >= $3 ORDER BY created_at DESC LIMIT $4"
                )
                .bind(team)
                .bind(dataplane_name)
                .bind(since_time)
                .bind(limit)
                .fetch_all(&self.pool)
                .await
            }
            None => {
                sqlx::query_as::<sqlx::Postgres, NackEventRow>(
                    "SELECT id, team, dataplane_name, type_url, version_rejected, nonce, error_code, error_message, node_id, resource_names, source, dedup_hash, created_at \
                     FROM xds_nack_events WHERE team = $1 AND dataplane_name = $2 ORDER BY created_at DESC LIMIT $3"
                )
                .bind(team)
                .bind(dataplane_name)
                .bind(limit)
                .fetch_all(&self.pool)
                .await
            }
        }
        .map_err(|e| {
            tracing::error!(error = %e, team = %team, dataplane = %dataplane_name, "Failed to list NACK events by dataplane");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list NACK events for dataplane '{}' in team '{}'", dataplane_name, team),
            }
        })?;

        rows.into_iter().map(NackEventData::try_from).collect()
    }

    /// Lists NACK events for a specific xDS resource type within a team.
    #[instrument(skip(self), fields(team = %team, type_url = %type_url, since = ?since, limit = ?limit), name = "db_list_nack_events_by_type_url")]
    pub async fn list_by_type_url(
        &self,
        team: &str,
        type_url: &str,
        since: Option<DateTime<Utc>>,
        limit: Option<i32>,
    ) -> Result<Vec<NackEventData>> {
        let limit = limit.unwrap_or(100).min(1000);

        let rows = match since {
            Some(since_time) => {
                sqlx::query_as::<sqlx::Postgres, NackEventRow>(
                    "SELECT id, team, dataplane_name, type_url, version_rejected, nonce, error_code, error_message, node_id, resource_names, source, dedup_hash, created_at \
                     FROM xds_nack_events WHERE team = $1 AND type_url = $2 AND created_at >= $3 ORDER BY created_at DESC LIMIT $4"
                )
                .bind(team)
                .bind(type_url)
                .bind(since_time)
                .bind(limit)
                .fetch_all(&self.pool)
                .await
            }
            None => {
                sqlx::query_as::<sqlx::Postgres, NackEventRow>(
                    "SELECT id, team, dataplane_name, type_url, version_rejected, nonce, error_code, error_message, node_id, resource_names, source, dedup_hash, created_at \
                     FROM xds_nack_events WHERE team = $1 AND type_url = $2 ORDER BY created_at DESC LIMIT $3"
                )
                .bind(team)
                .bind(type_url)
                .bind(limit)
                .fetch_all(&self.pool)
                .await
            }
        }
        .map_err(|e| {
            tracing::error!(error = %e, team = %team, type_url = %type_url, "Failed to list NACK events by type_url");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list NACK events for type_url '{}' in team '{}'", type_url, team),
            }
        })?;

        rows.into_iter().map(NackEventData::try_from).collect()
    }

    /// Lists recent NACK events for a team, optionally filtered by a start time.
    #[instrument(skip(self), fields(team = %team, since = ?since, limit = ?limit), name = "db_list_recent_nack_events")]
    pub async fn list_recent(
        &self,
        team: &str,
        since: Option<DateTime<Utc>>,
        limit: Option<i32>,
    ) -> Result<Vec<NackEventData>> {
        let limit = limit.unwrap_or(100).min(1000);

        let rows = match since {
            Some(since_time) => {
                sqlx::query_as::<sqlx::Postgres, NackEventRow>(
                    "SELECT id, team, dataplane_name, type_url, version_rejected, nonce, error_code, error_message, node_id, resource_names, source, dedup_hash, created_at \
                     FROM xds_nack_events WHERE team = $1 AND created_at >= $2 ORDER BY created_at DESC LIMIT $3"
                )
                .bind(team)
                .bind(since_time)
                .bind(limit)
                .fetch_all(&self.pool)
                .await
            }
            None => {
                sqlx::query_as::<sqlx::Postgres, NackEventRow>(
                    "SELECT id, team, dataplane_name, type_url, version_rejected, nonce, error_code, error_message, node_id, resource_names, source, dedup_hash, created_at \
                     FROM xds_nack_events WHERE team = $1 ORDER BY created_at DESC LIMIT $2"
                )
                .bind(team)
                .bind(limit)
                .fetch_all(&self.pool)
                .await
            }
        }
        .map_err(|e| {
            tracing::error!(error = %e, team = %team, "Failed to list recent NACK events");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list recent NACK events for team '{}'", team),
            }
        })?;

        rows.into_iter().map(NackEventData::try_from).collect()
    }

    /// Gets the most recent NACK per xDS resource type for a specific dataplane.
    ///
    /// Uses `DISTINCT ON` to return only the latest NACK for each `type_url`,
    /// giving a snapshot of the current NACK state per resource type.
    #[instrument(skip(self), fields(team = %team, dataplane = %dataplane_name), name = "db_latest_nack_per_type_url")]
    pub async fn latest_per_type_url(
        &self,
        team: &str,
        dataplane_name: &str,
    ) -> Result<Vec<NackEventData>> {
        let rows = sqlx::query_as::<sqlx::Postgres, NackEventRow>(
            "SELECT DISTINCT ON (type_url) id, team, dataplane_name, type_url, version_rejected, nonce, error_code, error_message, node_id, resource_names, source, dedup_hash, created_at \
             FROM xds_nack_events WHERE team = $1 AND dataplane_name = $2 ORDER BY type_url, created_at DESC"
        )
        .bind(team)
        .bind(dataplane_name)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, team = %team, dataplane = %dataplane_name, "Failed to get latest NACKs per type_url");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to get latest NACKs per type_url for dataplane '{}' in team '{}'", dataplane_name, team),
            }
        })?;

        rows.into_iter().map(NackEventData::try_from).collect()
    }

    /// Returns the database pool.
    pub fn pool(&self) -> &DbPool {
        &self.pool
    }
}

#[cfg(test)]
#[cfg(feature = "postgres_tests")]
mod tests {
    use super::*;
    use crate::storage::test_helpers::TestDatabase;

    fn make_request(
        team: &str,
        dataplane: &str,
        type_url: &str,
        error: &str,
    ) -> CreateNackEventRequest {
        CreateNackEventRequest {
            team: team.to_string(),
            dataplane_name: dataplane.to_string(),
            type_url: type_url.to_string(),
            version_rejected: Some("v1".to_string()),
            nonce: Some("nonce-1".to_string()),
            error_code: 2,
            error_message: error.to_string(),
            node_id: Some("envoy-node-1".to_string()),
            resource_names: Some(r#"["my-cluster"]"#.to_string()),
            source: NackSource::Stream,
            dedup_hash: None,
        }
    }

    #[tokio::test]
    async fn test_insert_and_retrieve() {
        let db = TestDatabase::new("nack_repo_insert").await;
        let repo = NackEventRepository::new(db.pool.clone());

        let req = make_request(
            crate::storage::test_helpers::TEST_TEAM_ID,
            "dp-1",
            "type.googleapis.com/envoy.config.cluster.v3.Cluster",
            "threshold missing",
        );
        let event = repo.insert(req).await.expect("insert should succeed");

        assert_eq!(event.team, crate::storage::test_helpers::TEST_TEAM_ID);
        assert_eq!(event.dataplane_name, "dp-1");
        assert_eq!(event.error_message, "threshold missing");
        assert!(!event.id.is_empty());
    }

    #[tokio::test]
    async fn test_list_by_team() {
        let db = TestDatabase::new("nack_repo_team").await;
        let repo = NackEventRepository::new(db.pool.clone());

        // Insert 2 events for team-a, 1 for team-b
        repo.insert(make_request(
            crate::storage::test_helpers::TEST_TEAM_ID,
            "dp-1",
            "type.googleapis.com/envoy.config.cluster.v3.Cluster",
            "err1",
        ))
        .await
        .expect("insert");
        repo.insert(make_request(
            crate::storage::test_helpers::TEST_TEAM_ID,
            "dp-2",
            "type.googleapis.com/envoy.config.listener.v3.Listener",
            "err2",
        ))
        .await
        .expect("insert");
        repo.insert(make_request(
            crate::storage::test_helpers::TEAM_A_ID,
            "dp-3",
            "type.googleapis.com/envoy.config.cluster.v3.Cluster",
            "err3",
        ))
        .await
        .expect("insert");

        let results = repo
            .list_by_team(crate::storage::test_helpers::TEST_TEAM_ID, None, None)
            .await
            .expect("list should succeed");
        assert_eq!(results.len(), 2, "should only see test-team events");
        assert!(results.iter().all(|e| e.team == crate::storage::test_helpers::TEST_TEAM_ID));
    }

    #[tokio::test]
    async fn test_list_by_dataplane() {
        let db = TestDatabase::new("nack_repo_dp").await;
        let repo = NackEventRepository::new(db.pool.clone());

        repo.insert(make_request(
            crate::storage::test_helpers::TEST_TEAM_ID,
            "dp-alpha",
            "type.googleapis.com/envoy.config.cluster.v3.Cluster",
            "err1",
        ))
        .await
        .expect("insert");
        repo.insert(make_request(
            crate::storage::test_helpers::TEST_TEAM_ID,
            "dp-alpha",
            "type.googleapis.com/envoy.config.listener.v3.Listener",
            "err2",
        ))
        .await
        .expect("insert");
        repo.insert(make_request(
            crate::storage::test_helpers::TEST_TEAM_ID,
            "dp-beta",
            "type.googleapis.com/envoy.config.cluster.v3.Cluster",
            "err3",
        ))
        .await
        .expect("insert");

        let results = repo
            .list_by_dataplane(crate::storage::test_helpers::TEST_TEAM_ID, "dp-alpha", None, None)
            .await
            .expect("list");
        assert_eq!(results.len(), 2, "should only see dp-alpha events");
        assert!(results.iter().all(|e| e.dataplane_name == "dp-alpha"));
    }

    #[tokio::test]
    async fn test_list_by_type_url() {
        let db = TestDatabase::new("nack_repo_type").await;
        let repo = NackEventRepository::new(db.pool.clone());

        let cds_url = "type.googleapis.com/envoy.config.cluster.v3.Cluster";
        let lds_url = "type.googleapis.com/envoy.config.listener.v3.Listener";

        repo.insert(make_request(
            crate::storage::test_helpers::TEST_TEAM_ID,
            "dp-1",
            cds_url,
            "cds err",
        ))
        .await
        .expect("insert");
        repo.insert(make_request(
            crate::storage::test_helpers::TEST_TEAM_ID,
            "dp-1",
            lds_url,
            "lds err",
        ))
        .await
        .expect("insert");
        repo.insert(make_request(
            crate::storage::test_helpers::TEST_TEAM_ID,
            "dp-2",
            cds_url,
            "cds err2",
        ))
        .await
        .expect("insert");

        let results = repo
            .list_by_type_url(crate::storage::test_helpers::TEST_TEAM_ID, cds_url, None, None)
            .await
            .expect("list");
        assert_eq!(results.len(), 2, "should only see CDS events");
        assert!(results.iter().all(|e| e.type_url == cds_url));
    }

    #[tokio::test]
    async fn test_team_isolation() {
        let db = TestDatabase::new("nack_repo_iso").await;
        let repo = NackEventRepository::new(db.pool.clone());

        repo.insert(make_request(
            crate::storage::test_helpers::TEAM_A_ID,
            "dp-1",
            "type.googleapis.com/envoy.config.cluster.v3.Cluster",
            "secret error",
        ))
        .await
        .expect("insert");

        let results = repo
            .list_by_team(crate::storage::test_helpers::TEAM_B_ID, None, None)
            .await
            .expect("list");
        assert!(results.is_empty(), "team-b should not see team-a events");

        let results = repo
            .list_by_dataplane(crate::storage::test_helpers::TEAM_B_ID, "dp-1", None, None)
            .await
            .expect("list");
        assert!(results.is_empty(), "team-b should not see team-a dataplane events");
    }

    #[tokio::test]
    async fn test_latest_per_type_url() {
        let db = TestDatabase::new("nack_repo_latest").await;
        let repo = NackEventRepository::new(db.pool.clone());

        let cds_url = "type.googleapis.com/envoy.config.cluster.v3.Cluster";
        let lds_url = "type.googleapis.com/envoy.config.listener.v3.Listener";

        // Insert 2 CDS events and 1 LDS event for the same dataplane
        repo.insert(make_request(
            crate::storage::test_helpers::TEST_TEAM_ID,
            "dp-1",
            cds_url,
            "old cds error",
        ))
        .await
        .expect("insert");
        repo.insert(make_request(
            crate::storage::test_helpers::TEST_TEAM_ID,
            "dp-1",
            cds_url,
            "new cds error",
        ))
        .await
        .expect("insert");
        repo.insert(make_request(
            crate::storage::test_helpers::TEST_TEAM_ID,
            "dp-1",
            lds_url,
            "lds error",
        ))
        .await
        .expect("insert");

        let results = repo
            .latest_per_type_url(crate::storage::test_helpers::TEST_TEAM_ID, "dp-1")
            .await
            .expect("latest");
        assert_eq!(results.len(), 2, "should have one entry per type_url");

        // Verify we got the latest CDS (not the old one)
        let cds_entry =
            results.iter().find(|e| e.type_url == cds_url).expect("should have CDS entry");
        assert_eq!(cds_entry.error_message, "new cds error");
    }

    #[tokio::test]
    async fn test_list_recent_with_limit() {
        let db = TestDatabase::new("nack_repo_recent").await;
        let repo = NackEventRepository::new(db.pool.clone());

        for i in 0..5 {
            repo.insert(make_request(
                crate::storage::test_helpers::TEST_TEAM_ID,
                "dp-1",
                "type.googleapis.com/envoy.config.cluster.v3.Cluster",
                &format!("err {}", i),
            ))
            .await
            .expect("insert");
        }

        let results = repo
            .list_recent(crate::storage::test_helpers::TEST_TEAM_ID, None, Some(3))
            .await
            .expect("list");
        assert_eq!(results.len(), 3, "should respect limit");
    }

    #[tokio::test]
    async fn test_list_by_dataplane_with_since() {
        let db = TestDatabase::new("nack_repo_dp_since").await;
        let repo = NackEventRepository::new(db.pool.clone());

        // Insert events
        repo.insert(make_request(
            crate::storage::test_helpers::TEST_TEAM_ID,
            "dp-1",
            "type.googleapis.com/envoy.config.cluster.v3.Cluster",
            "old err",
        ))
        .await
        .expect("insert");

        let cutoff = Utc::now();
        // Small delay to ensure timestamp ordering
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        repo.insert(make_request(
            crate::storage::test_helpers::TEST_TEAM_ID,
            "dp-1",
            "type.googleapis.com/envoy.config.cluster.v3.Cluster",
            "new err",
        ))
        .await
        .expect("insert");

        // Without since: should get both
        let all = repo
            .list_by_dataplane(crate::storage::test_helpers::TEST_TEAM_ID, "dp-1", None, None)
            .await
            .expect("list");
        assert_eq!(all.len(), 2);

        // With since: should only get the newer event
        let filtered = repo
            .list_by_dataplane(
                crate::storage::test_helpers::TEST_TEAM_ID,
                "dp-1",
                Some(cutoff),
                None,
            )
            .await
            .expect("list");
        assert_eq!(filtered.len(), 1, "since filter should exclude old events");
        assert_eq!(filtered[0].error_message, "new err");
    }

    /// fp-4g4 regression: an insert followed by `list_by_team` using the same
    /// team **id** must return the row. Before the migration, the repo stored
    /// team NAMES while the handler queried by team id, so this round-trip
    /// returned an empty list for any caller that resolved name → id first
    /// (which `flowplane xds nacks` does). This test would have caught the
    /// bug at compile-and-test time. Do not delete.
    #[tokio::test]
    async fn fp_4g4_round_trip_by_team_id() {
        let db = TestDatabase::new("nack_repo_fp4g4").await;
        let repo = NackEventRepository::new(db.pool.clone());

        let team_id = crate::storage::test_helpers::TEST_TEAM_ID;
        let inserted = repo
            .insert(make_request(
                team_id,
                "dp-warming",
                "type.googleapis.com/envoy.config.listener.v3.Listener",
                "warming failed: bind() EADDRINUSE",
            ))
            .await
            .expect("insert should succeed against teams.id FK");

        assert_eq!(inserted.team, team_id, "row stores team_id, not team name");

        let listed = repo.list_by_team(team_id, None, None).await.expect("list");
        assert_eq!(
            listed.len(),
            1,
            "list_by_team(<team_id>) must return rows inserted with the same team_id — \
             this is the exact contract that broke in fp-4g4"
        );
        assert_eq!(listed[0].id, inserted.id);
        assert_eq!(listed[0].error_message, "warming failed: bind() EADDRINUSE");
    }

    #[tokio::test]
    async fn test_list_by_type_url_with_since() {
        let db = TestDatabase::new("nack_repo_type_since").await;
        let repo = NackEventRepository::new(db.pool.clone());

        let cds_url = "type.googleapis.com/envoy.config.cluster.v3.Cluster";

        repo.insert(make_request(
            crate::storage::test_helpers::TEST_TEAM_ID,
            "dp-1",
            cds_url,
            "old cds err",
        ))
        .await
        .expect("insert");

        let cutoff = Utc::now();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        repo.insert(make_request(
            crate::storage::test_helpers::TEST_TEAM_ID,
            "dp-2",
            cds_url,
            "new cds err",
        ))
        .await
        .expect("insert");

        // Without since: should get both
        let all = repo
            .list_by_type_url(crate::storage::test_helpers::TEST_TEAM_ID, cds_url, None, None)
            .await
            .expect("list");
        assert_eq!(all.len(), 2);

        // With since: should only get the newer event
        let filtered = repo
            .list_by_type_url(
                crate::storage::test_helpers::TEST_TEAM_ID,
                cds_url,
                Some(cutoff),
                None,
            )
            .await
            .expect("list");
        assert_eq!(filtered.len(), 1, "since filter should exclude old events");
        assert_eq!(filtered[0].error_message, "new cds err");
    }
}
