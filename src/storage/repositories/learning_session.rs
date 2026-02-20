//! Learning session repository for managing API schema learning lifecycle
//!
//! This module provides CRUD operations for learning session resources, handling storage,
//! retrieval, and lifecycle management of learning session data.

use crate::errors::{FlowplaneError, Result};
use crate::storage::DbPool;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Row};
use tracing::instrument;

/// Learning session status
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LearningSessionStatus {
    Pending,
    Active,
    Completing,
    Completed,
    Cancelled,
    Failed,
}

impl std::fmt::Display for LearningSessionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Active => write!(f, "active"),
            Self::Completing => write!(f, "completing"),
            Self::Completed => write!(f, "completed"),
            Self::Cancelled => write!(f, "cancelled"),
            Self::Failed => write!(f, "failed"),
        }
    }
}

impl std::str::FromStr for LearningSessionStatus {
    type Err = FlowplaneError;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "pending" => Ok(Self::Pending),
            "active" => Ok(Self::Active),
            "completing" => Ok(Self::Completing),
            "completed" => Ok(Self::Completed),
            "cancelled" => Ok(Self::Cancelled),
            "failed" => Ok(Self::Failed),
            _ => Err(FlowplaneError::validation(format!("Invalid session status: {}", s))),
        }
    }
}

/// Database row structure for learning sessions
#[derive(Debug, Clone, FromRow)]
struct LearningSessionRow {
    pub id: String,
    pub team: String,
    pub route_pattern: String,
    pub cluster_name: Option<String>,
    pub http_methods: Option<String>, // JSON array
    pub status: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub ends_at: Option<chrono::DateTime<chrono::Utc>>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub target_sample_count: i64,
    pub current_sample_count: i64,
    pub triggered_by: Option<String>,
    pub deployment_version: Option<String>,
    pub configuration_snapshot: Option<String>, // JSON
    pub error_message: Option<String>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Learning session data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningSessionData {
    pub id: String,
    pub team: String,
    pub route_pattern: String,
    pub cluster_name: Option<String>,
    pub http_methods: Option<Vec<String>>,
    pub status: LearningSessionStatus,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub ends_at: Option<chrono::DateTime<chrono::Utc>>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub target_sample_count: i64,
    pub current_sample_count: i64,
    pub triggered_by: Option<String>,
    pub deployment_version: Option<String>,
    pub configuration_snapshot: Option<serde_json::Value>,
    pub error_message: Option<String>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl TryFrom<LearningSessionRow> for LearningSessionData {
    type Error = FlowplaneError;

    fn try_from(row: LearningSessionRow) -> Result<Self> {
        let status = row.status.parse()?;

        let http_methods = row
            .http_methods
            .as_ref()
            .map(|json_str| {
                serde_json::from_str::<Vec<String>>(json_str).map_err(|e| {
                    FlowplaneError::validation(format!("Invalid http_methods JSON: {}", e))
                })
            })
            .transpose()?;

        let configuration_snapshot = row
            .configuration_snapshot
            .as_ref()
            .map(|json_str| {
                serde_json::from_str::<serde_json::Value>(json_str).map_err(|e| {
                    FlowplaneError::validation(format!(
                        "Invalid configuration_snapshot JSON: {}",
                        e
                    ))
                })
            })
            .transpose()?;

        Ok(Self {
            id: row.id,
            team: row.team,
            route_pattern: row.route_pattern,
            cluster_name: row.cluster_name,
            http_methods,
            status,
            created_at: row.created_at,
            started_at: row.started_at,
            ends_at: row.ends_at,
            completed_at: row.completed_at,
            target_sample_count: row.target_sample_count,
            current_sample_count: row.current_sample_count,
            triggered_by: row.triggered_by,
            deployment_version: row.deployment_version,
            configuration_snapshot,
            error_message: row.error_message,
            updated_at: row.updated_at,
        })
    }
}

impl crate::api::handlers::TeamOwned for LearningSessionData {
    fn team(&self) -> Option<&str> {
        Some(&self.team)
    }

    fn resource_name(&self) -> &str {
        &self.id
    }

    fn resource_type() -> &'static str {
        "Learning session"
    }

    fn resource_type_metric() -> &'static str {
        "learning_sessions"
    }

    fn identifier_label() -> &'static str {
        "ID"
    }
}

/// Create learning session request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateLearningSessionRequest {
    pub team: String,
    pub route_pattern: String,
    pub cluster_name: Option<String>,
    pub http_methods: Option<Vec<String>>,
    pub target_sample_count: i64,
    pub max_duration_seconds: Option<i64>,
    pub triggered_by: Option<String>,
    pub deployment_version: Option<String>,
    pub configuration_snapshot: Option<serde_json::Value>,
}

/// Update learning session request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateLearningSessionRequest {
    pub status: Option<LearningSessionStatus>,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub ends_at: Option<chrono::DateTime<chrono::Utc>>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub current_sample_count: Option<i64>,
    pub error_message: Option<String>,
}

/// Repository for learning session data access
#[derive(Debug, Clone)]
pub struct LearningSessionRepository {
    pool: DbPool,
}

impl LearningSessionRepository {
    /// Create a new learning session repository
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Create a new learning session
    #[instrument(skip(self, request), fields(team = %request.team, route_pattern = %request.route_pattern), name = "db_create_learning_session")]
    pub async fn create(
        &self,
        request: CreateLearningSessionRequest,
    ) -> Result<LearningSessionData> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now();

        let http_methods_json = request
            .http_methods
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| FlowplaneError::validation(format!("Invalid http_methods: {}", e)))?;

        let config_snapshot_json = request
            .configuration_snapshot
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| {
                FlowplaneError::validation(format!("Invalid configuration_snapshot: {}", e))
            })?;

        let ends_at =
            request.max_duration_seconds.map(|seconds| now + chrono::Duration::seconds(seconds));

        let result = sqlx::query(
            "INSERT INTO learning_sessions (
                id, team, route_pattern, cluster_name, http_methods, status,
                created_at, ends_at, target_sample_count, current_sample_count,
                triggered_by, deployment_version, configuration_snapshot, updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
        )
        .bind(&id)
        .bind(&request.team)
        .bind(&request.route_pattern)
        .bind(&request.cluster_name)
        .bind(&http_methods_json)
        .bind(LearningSessionStatus::Pending.to_string())
        .bind(now)
        .bind(ends_at)
        .bind(request.target_sample_count)
        .bind(0i64) // current_sample_count starts at 0
        .bind(&request.triggered_by)
        .bind(&request.deployment_version)
        .bind(&config_snapshot_json)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, team = %request.team, "Failed to create learning session");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to create learning session for team '{}'", request.team),
            }
        })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::validation("Failed to create learning session"));
        }

        tracing::info!(
            session_id = %id,
            team = %request.team,
            route_pattern = %request.route_pattern,
            "Created new learning session"
        );

        self.get_by_id(&id).await
    }

    /// Get learning session by ID
    #[instrument(skip(self), fields(session_id = %id), name = "db_get_learning_session_by_id")]
    pub async fn get_by_id(&self, id: &str) -> Result<LearningSessionData> {
        let row = sqlx::query_as::<sqlx::Postgres, LearningSessionRow>(
            "SELECT id, team, route_pattern, cluster_name, http_methods, status,
                    created_at, started_at, ends_at, completed_at,
                    target_sample_count, current_sample_count,
                    triggered_by, deployment_version, configuration_snapshot,
                    error_message, updated_at
             FROM learning_sessions WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, session_id = %id, "Failed to get learning session by ID");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to get learning session with ID '{}'", id),
            }
        })?;

        match row {
            Some(row) => row.try_into(),
            None => Err(FlowplaneError::not_found_msg(format!(
                "Learning session with ID '{}' not found",
                id
            ))),
        }
    }

    /// Get learning session by ID and team (for authorization)
    #[instrument(skip(self), fields(session_id = %id, team = %team), name = "db_get_learning_session_by_id_and_team")]
    pub async fn get_by_id_and_team(&self, id: &str, team: &str) -> Result<LearningSessionData> {
        let row = sqlx::query_as::<sqlx::Postgres, LearningSessionRow>(
            "SELECT id, team, route_pattern, cluster_name, http_methods, status,
                    created_at, started_at, ends_at, completed_at,
                    target_sample_count, current_sample_count,
                    triggered_by, deployment_version, configuration_snapshot,
                    error_message, updated_at
             FROM learning_sessions WHERE id = $1 AND team = $2"
        )
        .bind(id)
        .bind(team)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, session_id = %id, team = %team, "Failed to get learning session");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to get learning session with ID '{}' for team '{}'", id, team),
            }
        })?;

        match row {
            Some(row) => row.try_into(),
            None => Err(FlowplaneError::not_found_msg(format!(
                "Learning session with ID '{}' not found for team '{}'",
                id, team
            ))),
        }
    }

    /// List learning sessions by team
    #[instrument(skip(self), fields(team = %team), name = "db_list_learning_sessions_by_team")]
    pub async fn list_by_team(
        &self,
        team: &str,
        status_filter: Option<LearningSessionStatus>,
        limit: Option<i32>,
        offset: Option<i32>,
    ) -> Result<Vec<LearningSessionData>> {
        let limit = limit.unwrap_or(100).min(1000);
        let offset = offset.unwrap_or(0);

        let (query, status_str) = match status_filter {
            Some(status) => (
                "SELECT id, team, route_pattern, cluster_name, http_methods, status,
                        created_at, started_at, ends_at, completed_at,
                        target_sample_count, current_sample_count,
                        triggered_by, deployment_version, configuration_snapshot,
                        error_message, updated_at
                 FROM learning_sessions
                 WHERE team = $1 AND status = $2
                 ORDER BY created_at DESC LIMIT $3 OFFSET $4",
                Some(status.to_string()),
            ),
            None => (
                "SELECT id, team, route_pattern, cluster_name, http_methods, status,
                        created_at, started_at, ends_at, completed_at,
                        target_sample_count, current_sample_count,
                        triggered_by, deployment_version, configuration_snapshot,
                        error_message, updated_at
                 FROM learning_sessions
                 WHERE team = $1
                 ORDER BY created_at DESC LIMIT $2 OFFSET $3",
                None,
            ),
        };

        let rows = if let Some(status_str) = status_str {
            sqlx::query_as::<sqlx::Postgres, LearningSessionRow>(query)
                .bind(team)
                .bind(status_str)
                .bind(limit)
                .bind(offset)
                .fetch_all(&self.pool)
                .await
        } else {
            sqlx::query_as::<sqlx::Postgres, LearningSessionRow>(query)
                .bind(team)
                .bind(limit)
                .bind(offset)
                .fetch_all(&self.pool)
                .await
        }
        .map_err(|e| {
            tracing::error!(error = %e, team = %team, "Failed to list learning sessions");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list learning sessions for team '{}'", team),
            }
        })?;

        rows.into_iter().map(|row| row.try_into()).collect()
    }

    /// List all learning sessions across all teams (for admin users)
    ///
    /// This method bypasses team filtering and returns sessions from all teams.
    /// Should only be used by users with admin:all scope.
    #[instrument(skip(self), name = "db_list_all_learning_sessions")]
    pub async fn list_all(
        &self,
        status_filter: Option<LearningSessionStatus>,
        limit: Option<i32>,
        offset: Option<i32>,
    ) -> Result<Vec<LearningSessionData>> {
        let limit = limit.unwrap_or(100).min(1000);
        let offset = offset.unwrap_or(0);

        let (query, status_str) = match status_filter {
            Some(status) => (
                "SELECT id, team, route_pattern, cluster_name, http_methods, status,
                        created_at, started_at, ends_at, completed_at,
                        target_sample_count, current_sample_count,
                        triggered_by, deployment_version, configuration_snapshot,
                        error_message, updated_at
                 FROM learning_sessions
                 WHERE status = $1
                 ORDER BY created_at DESC
                 LIMIT $2 OFFSET $3",
                Some(status.to_string()),
            ),
            None => (
                "SELECT id, team, route_pattern, cluster_name, http_methods, status,
                        created_at, started_at, ends_at, completed_at,
                        target_sample_count, current_sample_count,
                        triggered_by, deployment_version, configuration_snapshot,
                        error_message, updated_at
                 FROM learning_sessions
                 ORDER BY created_at DESC
                 LIMIT $1 OFFSET $2",
                None,
            ),
        };

        let rows: Vec<LearningSessionRow> = if let Some(status) = status_str {
            sqlx::query_as::<sqlx::Postgres, LearningSessionRow>(query)
                .bind(status)
                .bind(limit)
                .bind(offset)
                .fetch_all(&self.pool)
                .await
        } else {
            sqlx::query_as::<sqlx::Postgres, LearningSessionRow>(query)
                .bind(limit)
                .bind(offset)
                .fetch_all(&self.pool)
                .await
        }
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to list all learning sessions");
            FlowplaneError::Database {
                source: e,
                context: "Failed to list all learning sessions".to_string(),
            }
        })?;

        rows.into_iter().map(|row| row.try_into()).collect()
    }

    /// List all active learning sessions (for Access Log Service)
    #[instrument(skip(self), name = "db_list_active_learning_sessions")]
    pub async fn list_active(&self) -> Result<Vec<LearningSessionData>> {
        let rows = sqlx::query_as::<sqlx::Postgres, LearningSessionRow>(
            "SELECT id, team, route_pattern, cluster_name, http_methods, status,
                    created_at, started_at, ends_at, completed_at,
                    target_sample_count, current_sample_count,
                    triggered_by, deployment_version, configuration_snapshot,
                    error_message, updated_at
             FROM learning_sessions
             WHERE status = $1
             ORDER BY created_at DESC",
        )
        .bind(LearningSessionStatus::Active.to_string())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to list active learning sessions");
            FlowplaneError::Database {
                source: e,
                context: "Failed to list active learning sessions".to_string(),
            }
        })?;

        rows.into_iter().map(|row| row.try_into()).collect()
    }

    /// Update learning session
    #[instrument(skip(self, request), fields(session_id = %id), name = "db_update_learning_session")]
    pub async fn update(
        &self,
        id: &str,
        request: UpdateLearningSessionRequest,
    ) -> Result<LearningSessionData> {
        let now = chrono::Utc::now();

        // Build dynamic update query based on what's provided
        let mut updates = vec!["updated_at = $1".to_string()];
        let mut bind_count = 2;

        if request.status.is_some() {
            updates.push(format!("status = ${}", bind_count));
            bind_count += 1;
        }
        if request.started_at.is_some() {
            updates.push(format!("started_at = ${}", bind_count));
            bind_count += 1;
        }
        if request.ends_at.is_some() {
            updates.push(format!("ends_at = ${}", bind_count));
            bind_count += 1;
        }
        if request.completed_at.is_some() {
            updates.push(format!("completed_at = ${}", bind_count));
            bind_count += 1;
        }
        if request.current_sample_count.is_some() {
            updates.push(format!("current_sample_count = ${}", bind_count));
            bind_count += 1;
        }
        if request.error_message.is_some() {
            updates.push(format!("error_message = ${}", bind_count));
            bind_count += 1;
        }

        let query = format!(
            "UPDATE learning_sessions SET {} WHERE id = ${}",
            updates.join(", "),
            bind_count
        );

        let mut q = sqlx::query(&query).bind(now);

        if let Some(status) = request.status {
            q = q.bind(status.to_string());
        }
        if let Some(started_at) = request.started_at {
            q = q.bind(started_at);
        }
        if let Some(ends_at) = request.ends_at {
            q = q.bind(ends_at);
        }
        if let Some(completed_at) = request.completed_at {
            q = q.bind(completed_at);
        }
        if let Some(count) = request.current_sample_count {
            q = q.bind(count);
        }
        if let Some(error) = request.error_message {
            q = q.bind(error);
        }

        q = q.bind(id);

        let result = q.execute(&self.pool).await.map_err(|e| {
            tracing::error!(error = %e, session_id = %id, "Failed to update learning session");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to update learning session '{}'", id),
            }
        })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::not_found_msg(format!(
                "Learning session with ID '{}' not found",
                id
            )));
        }

        self.get_by_id(id).await
    }

    /// Atomically transition session status with conditional check
    ///
    /// This method uses a conditional UPDATE to prevent race conditions.
    /// The transition only succeeds if the current status matches `from_status`.
    ///
    /// # Returns
    /// - `Ok(true)` if transition succeeded (rows affected = 1)
    /// - `Ok(false)` if transition failed (status was already changed by another caller)
    /// - `Err` if database error occurred
    ///
    /// This method is safe for concurrent calls - only one caller will succeed in
    /// transitioning from a given status.
    #[instrument(skip(self), fields(session_id = %id, from = %from_status, to = %to_status), name = "db_transition_session_status")]
    pub async fn transition_status(
        &self,
        id: &str,
        from_status: LearningSessionStatus,
        to_status: LearningSessionStatus,
    ) -> Result<bool> {
        let now = chrono::Utc::now();

        let result = sqlx::query(
            "UPDATE learning_sessions
             SET status = $1, updated_at = $2
             WHERE id = $3 AND status = $4",
        )
        .bind(to_status.to_string())
        .bind(now)
        .bind(id)
        .bind(from_status.to_string())
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(
                error = %e,
                session_id = %id,
                from_status = %from_status,
                to_status = %to_status,
                "Failed to transition session status"
            );
            FlowplaneError::Database {
                source: e,
                context: format!(
                    "Failed to transition session '{}' from {} to {}",
                    id, from_status, to_status
                ),
            }
        })?;

        Ok(result.rows_affected() > 0)
    }

    /// Increment sample count
    #[instrument(skip(self), fields(session_id = %id), name = "db_increment_sample_count")]
    pub async fn increment_sample_count(&self, id: &str) -> Result<i64> {
        let now = chrono::Utc::now();

        let result = sqlx::query(
            "UPDATE learning_sessions
             SET current_sample_count = current_sample_count + 1, updated_at = $1
             WHERE id = $2
             RETURNING current_sample_count",
        )
        .bind(now)
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, session_id = %id, "Failed to increment sample count");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to increment sample count for session '{}'", id),
            }
        })?;

        match result {
            Some(row) => {
                let count: i64 = row.try_get("current_sample_count").map_err(|e| {
                    FlowplaneError::validation(format!("Failed to get current_sample_count: {}", e))
                })?;
                Ok(count)
            }
            None => Err(FlowplaneError::not_found_msg(format!(
                "Learning session with ID '{}' not found",
                id
            ))),
        }
    }

    /// Delete learning session (cancel)
    #[instrument(skip(self), fields(session_id = %id), name = "db_delete_learning_session")]
    pub async fn delete(&self, id: &str, team: &str) -> Result<()> {
        let result = sqlx::query("DELETE FROM learning_sessions WHERE id = $1 AND team = $2")
            .bind(id)
            .bind(team)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, session_id = %id, team = %team, "Failed to delete learning session");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to delete learning session '{}'", id),
                }
            })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::not_found_msg(format!(
                "Learning session with ID '{}' not found for team '{}'",
                id, team
            )));
        }

        tracing::info!(session_id = %id, team = %team, "Deleted learning session");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_status_to_string() {
        assert_eq!(LearningSessionStatus::Pending.to_string(), "pending");
        assert_eq!(LearningSessionStatus::Active.to_string(), "active");
        assert_eq!(LearningSessionStatus::Completing.to_string(), "completing");
        assert_eq!(LearningSessionStatus::Completed.to_string(), "completed");
        assert_eq!(LearningSessionStatus::Cancelled.to_string(), "cancelled");
        assert_eq!(LearningSessionStatus::Failed.to_string(), "failed");
    }

    #[test]
    fn test_session_status_from_string() {
        assert_eq!(
            "pending".parse::<LearningSessionStatus>().unwrap(),
            LearningSessionStatus::Pending
        );
        assert_eq!(
            "active".parse::<LearningSessionStatus>().unwrap(),
            LearningSessionStatus::Active
        );
        assert_eq!(
            "completing".parse::<LearningSessionStatus>().unwrap(),
            LearningSessionStatus::Completing
        );
        assert_eq!(
            "completed".parse::<LearningSessionStatus>().unwrap(),
            LearningSessionStatus::Completed
        );
        assert_eq!(
            "cancelled".parse::<LearningSessionStatus>().unwrap(),
            LearningSessionStatus::Cancelled
        );
        assert_eq!(
            "failed".parse::<LearningSessionStatus>().unwrap(),
            LearningSessionStatus::Failed
        );

        // Test case insensitivity
        assert_eq!(
            "PENDING".parse::<LearningSessionStatus>().unwrap(),
            LearningSessionStatus::Pending
        );
        assert_eq!(
            "Active".parse::<LearningSessionStatus>().unwrap(),
            LearningSessionStatus::Active
        );

        // Test invalid
        assert!("invalid".parse::<LearningSessionStatus>().is_err());
    }

    #[cfg(feature = "postgres_tests")]
    mod postgres_tests {
        use super::*;
        use crate::storage::test_helpers::{TestDatabase, TEAM_A_ID, TEST_TEAM_ID};

        #[tokio::test]
        async fn test_create_and_get_learning_session() {
            let test_db = TestDatabase::new("ls_create_get").await;
            let pool = test_db.pool.clone();
            let repo = LearningSessionRepository::new(pool);

            let request = CreateLearningSessionRequest {
                team: TEST_TEAM_ID.to_string(),
                route_pattern: "/api/users/*".to_string(),
                cluster_name: Some("test-cluster".to_string()),
                http_methods: Some(vec!["GET".to_string(), "POST".to_string()]),
                target_sample_count: 50,
                max_duration_seconds: Some(3600),
                triggered_by: Some("test-user".to_string()),
                deployment_version: Some("v1.0.0".to_string()),
                configuration_snapshot: Some(serde_json::json!({"key": "value"})),
            };

            let created = repo.create(request).await.expect("create should succeed");

            assert_eq!(created.team, TEST_TEAM_ID);
            assert_eq!(created.route_pattern, "/api/users/*");
            assert_eq!(created.cluster_name, Some("test-cluster".to_string()));
            assert_eq!(created.http_methods, Some(vec!["GET".to_string(), "POST".to_string()]));
            assert_eq!(created.status, LearningSessionStatus::Pending);
            assert_eq!(created.target_sample_count, 50);
            assert_eq!(created.current_sample_count, 0);
            assert_eq!(created.triggered_by, Some("test-user".to_string()));
            assert_eq!(created.deployment_version, Some("v1.0.0".to_string()));
            assert!(created.ends_at.is_some());
            assert!(created.configuration_snapshot.is_some());

            // Get it back by ID
            let fetched = repo.get_by_id(&created.id).await.expect("get_by_id should succeed");
            assert_eq!(fetched.id, created.id);
            assert_eq!(fetched.team, created.team);
            assert_eq!(fetched.route_pattern, created.route_pattern);
            assert_eq!(fetched.status, LearningSessionStatus::Pending);
        }

        #[tokio::test]
        async fn test_list_by_team() {
            let test_db = TestDatabase::new("ls_list_by_team").await;
            let pool = test_db.pool.clone();
            let repo = LearningSessionRepository::new(pool);

            // Create 2 sessions for TEST_TEAM_ID
            for i in 0..2 {
                let request = CreateLearningSessionRequest {
                    team: TEST_TEAM_ID.to_string(),
                    route_pattern: format!("/api/route-{}", i),
                    cluster_name: None,
                    http_methods: None,
                    target_sample_count: 10,
                    max_duration_seconds: None,
                    triggered_by: None,
                    deployment_version: None,
                    configuration_snapshot: None,
                };
                repo.create(request).await.expect("create should succeed");
            }

            // Create 1 session for TEAM_A_ID
            let request = CreateLearningSessionRequest {
                team: TEAM_A_ID.to_string(),
                route_pattern: "/api/team-a-route".to_string(),
                cluster_name: None,
                http_methods: None,
                target_sample_count: 10,
                max_duration_seconds: None,
                triggered_by: None,
                deployment_version: None,
                configuration_snapshot: None,
            };
            repo.create(request).await.expect("create should succeed");

            let test_team_sessions = repo
                .list_by_team(TEST_TEAM_ID, None, None, None)
                .await
                .expect("list_by_team should succeed");
            assert_eq!(test_team_sessions.len(), 2);

            let team_a_sessions = repo
                .list_by_team(TEAM_A_ID, None, None, None)
                .await
                .expect("list_by_team should succeed");
            assert_eq!(team_a_sessions.len(), 1);
        }

        #[tokio::test]
        async fn test_transition_status() {
            let test_db = TestDatabase::new("ls_transition_status").await;
            let pool = test_db.pool.clone();
            let repo = LearningSessionRepository::new(pool);

            let request = CreateLearningSessionRequest {
                team: TEST_TEAM_ID.to_string(),
                route_pattern: "/api/status-test".to_string(),
                cluster_name: None,
                http_methods: None,
                target_sample_count: 10,
                max_duration_seconds: None,
                triggered_by: None,
                deployment_version: None,
                configuration_snapshot: None,
            };
            let session = repo.create(request).await.expect("create should succeed");
            assert_eq!(session.status, LearningSessionStatus::Pending);

            // Pending -> Active
            let ok = repo
                .transition_status(
                    &session.id,
                    LearningSessionStatus::Pending,
                    LearningSessionStatus::Active,
                )
                .await
                .expect("transition should succeed");
            assert!(ok, "Pending -> Active should succeed");
            let fetched = repo.get_by_id(&session.id).await.unwrap();
            assert_eq!(fetched.status, LearningSessionStatus::Active);

            // Active -> Completing
            let ok = repo
                .transition_status(
                    &session.id,
                    LearningSessionStatus::Active,
                    LearningSessionStatus::Completing,
                )
                .await
                .expect("transition should succeed");
            assert!(ok, "Active -> Completing should succeed");
            let fetched = repo.get_by_id(&session.id).await.unwrap();
            assert_eq!(fetched.status, LearningSessionStatus::Completing);

            // Completing -> Completed
            let ok = repo
                .transition_status(
                    &session.id,
                    LearningSessionStatus::Completing,
                    LearningSessionStatus::Completed,
                )
                .await
                .expect("transition should succeed");
            assert!(ok, "Completing -> Completed should succeed");
            let fetched = repo.get_by_id(&session.id).await.unwrap();
            assert_eq!(fetched.status, LearningSessionStatus::Completed);

            // Wrong from_status should return false (not an error)
            let ok = repo
                .transition_status(
                    &session.id,
                    LearningSessionStatus::Pending,
                    LearningSessionStatus::Active,
                )
                .await
                .expect("transition should succeed (but not match)");
            assert!(!ok, "Wrong from_status should return false");
        }

        #[tokio::test]
        async fn test_increment_sample_count() {
            let test_db = TestDatabase::new("ls_increment_count").await;
            let pool = test_db.pool.clone();
            let repo = LearningSessionRepository::new(pool);

            let request = CreateLearningSessionRequest {
                team: TEST_TEAM_ID.to_string(),
                route_pattern: "/api/count-test".to_string(),
                cluster_name: None,
                http_methods: None,
                target_sample_count: 10,
                max_duration_seconds: None,
                triggered_by: None,
                deployment_version: None,
                configuration_snapshot: None,
            };
            let session = repo.create(request).await.expect("create should succeed");
            assert_eq!(session.current_sample_count, 0);

            // Increment 3 times
            let count = repo.increment_sample_count(&session.id).await.expect("increment 1");
            assert_eq!(count, 1);
            let count = repo.increment_sample_count(&session.id).await.expect("increment 2");
            assert_eq!(count, 2);
            let count = repo.increment_sample_count(&session.id).await.expect("increment 3");
            assert_eq!(count, 3);

            // Verify via get_by_id
            let fetched = repo.get_by_id(&session.id).await.unwrap();
            assert_eq!(fetched.current_sample_count, 3);
        }

        #[tokio::test]
        async fn test_delete_learning_session() {
            let test_db = TestDatabase::new("ls_delete").await;
            let pool = test_db.pool.clone();
            let repo = LearningSessionRepository::new(pool);

            let request = CreateLearningSessionRequest {
                team: TEST_TEAM_ID.to_string(),
                route_pattern: "/api/delete-test".to_string(),
                cluster_name: None,
                http_methods: None,
                target_sample_count: 10,
                max_duration_seconds: None,
                triggered_by: None,
                deployment_version: None,
                configuration_snapshot: None,
            };
            let session = repo.create(request).await.expect("create should succeed");

            // Verify it exists
            repo.get_by_id(&session.id).await.expect("session should exist before delete");

            // Delete it
            repo.delete(&session.id, TEST_TEAM_ID).await.expect("delete should succeed");

            // Verify get_by_id returns not found
            let result = repo.get_by_id(&session.id).await;
            assert!(result.is_err(), "get_by_id should return error after delete");
        }
    }
}
