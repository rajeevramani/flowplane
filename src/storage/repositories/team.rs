//! Team repository for team lifecycle management
//!
//! This module provides CRUD operations for teams, ensuring referential integrity
//! with team memberships and resource ownership.

use crate::auth::team::{CreateTeamRequest, Team, TeamStatus, UpdateTeamRequest};
use crate::domain::{OrgId, TeamId};
use crate::errors::{FlowplaneError, Result};
use crate::storage::DbPool;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::FromRow;
use std::str::FromStr;
use tracing::instrument;

// Database row structure

#[derive(Debug, Clone, FromRow)]
struct TeamRow {
    pub id: String,
    pub name: String,
    pub display_name: String,
    pub description: Option<String>,
    pub owner_user_id: Option<String>,
    pub org_id: Option<String>,
    pub settings: Option<String>, // JSON stored as string
    pub status: String,
    pub envoy_admin_port: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl TryFrom<TeamRow> for Team {
    type Error = FlowplaneError;

    fn try_from(row: TeamRow) -> Result<Self> {
        let status = TeamStatus::from_str(&row.status).map_err(|e| {
            FlowplaneError::validation(format!("Invalid team status '{}': {}", row.status, e))
        })?;

        let settings = if let Some(json_str) = row.settings {
            Some(serde_json::from_str(&json_str).map_err(|e| {
                FlowplaneError::validation(format!("Invalid team settings JSON: {}", e))
            })?)
        } else {
            None
        };

        Ok(Team {
            id: TeamId::from_string(row.id),
            name: row.name,
            display_name: row.display_name,
            description: row.description,
            owner_user_id: row.owner_user_id.map(|id| id.into()),
            org_id: row.org_id.map(|id| id.into()),
            settings,
            status,
            envoy_admin_port: row.envoy_admin_port.map(|p| p as u16),
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

// Repository trait

#[async_trait]
pub trait TeamRepository: Send + Sync {
    /// Create a new team
    async fn create_team(&self, request: CreateTeamRequest) -> Result<Team>;

    /// Get a team by ID
    async fn get_team_by_id(&self, id: &TeamId) -> Result<Option<Team>>;

    /// Get a team by name (unique)
    async fn get_team_by_name(&self, name: &str) -> Result<Option<Team>>;

    /// List all teams (with pagination)
    async fn list_teams(&self, limit: i64, offset: i64) -> Result<Vec<Team>>;

    /// List teams by status
    async fn list_teams_by_status(
        &self,
        status: TeamStatus,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Team>>;

    /// Count total teams
    async fn count_teams(&self) -> Result<i64>;

    /// Update a team's details
    async fn update_team(&self, id: &TeamId, update: UpdateTeamRequest) -> Result<Team>;

    /// Delete a team (will fail if there are resources referencing it due to FK RESTRICT)
    async fn delete_team(&self, id: &TeamId) -> Result<()>;

    /// Check if a team name is available
    async fn is_name_available(&self, name: &str) -> Result<bool>;

    /// List all teams belonging to an organization
    async fn list_teams_by_org(&self, org_id: &OrgId) -> Result<Vec<Team>>;

    /// Resolve team names to team IDs (UUIDs).
    /// Returns empty vec for empty input (admin bypass preserved).
    /// Errors if any team name doesn't exist.
    async fn resolve_team_ids(&self, team_names: &[String]) -> Result<Vec<String>>;
}

// SQLx implementation

#[derive(Debug)]
pub struct SqlxTeamRepository {
    pool: DbPool,
}

impl SqlxTeamRepository {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl TeamRepository for SqlxTeamRepository {
    #[instrument(skip(self, request), fields(team_name = %request.name), name = "db_create_team")]
    async fn create_team(&self, request: CreateTeamRequest) -> Result<Team> {
        let id = TeamId::new();
        let now = Utc::now();
        let settings_json = request
            .settings
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| FlowplaneError::validation(format!("Invalid settings JSON: {}", e)))?;

        // Auto-allocate Envoy admin port: MAX(existing) + 1, or base port if none exist
        let base_port = crate::config::DEFAULT_ENVOY_ADMIN_BASE_PORT as i64;
        let next_port: i64 =
            sqlx::query_scalar("SELECT COALESCE(MAX(envoy_admin_port), $1 - 1) + 1 FROM teams")
                .bind(base_port)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| FlowplaneError::Database {
                    source: e,
                    context: "Failed to allocate Envoy admin port".to_string(),
                })?;

        let row = sqlx::query_as::<_, TeamRow>(
            "INSERT INTO teams (
                id, name, display_name, description, owner_user_id, org_id, settings, status, envoy_admin_port, created_at, updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
            RETURNING *"
        )
        .bind(id.as_str())
        .bind(&request.name)
        .bind(&request.display_name)
        .bind(&request.description)
        .bind(request.owner_user_id.as_ref().map(|id| id.as_str()))
        .bind(request.org_id.as_ref().map(|id| id.as_str()))
        .bind(settings_json.as_deref())
        .bind(TeamStatus::Active.as_str())
        .bind(next_port)
        .bind(now)
        .bind(now)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: "Failed to create team".to_string(),
        })?;

        row.try_into()
    }

    #[instrument(skip(self), fields(team_id = %id), name = "db_get_team_by_id")]
    async fn get_team_by_id(&self, id: &TeamId) -> Result<Option<Team>> {
        let row = sqlx::query_as::<_, TeamRow>("SELECT * FROM teams WHERE id = $1")
            .bind(id.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| FlowplaneError::Database {
                source: e,
                context: format!("Failed to fetch team by ID: {}", id),
            })?;

        row.map(|r| r.try_into()).transpose()
    }

    #[instrument(skip(self), fields(team_name = %name), name = "db_get_team_by_name")]
    async fn get_team_by_name(&self, name: &str) -> Result<Option<Team>> {
        let row = sqlx::query_as::<_, TeamRow>("SELECT * FROM teams WHERE name = $1")
            .bind(name)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| FlowplaneError::Database {
                source: e,
                context: format!("Failed to fetch team by name: {}", name),
            })?;

        row.map(|r| r.try_into()).transpose()
    }

    #[instrument(skip(self), fields(limit = limit, offset = offset), name = "db_list_teams")]
    async fn list_teams(&self, limit: i64, offset: i64) -> Result<Vec<Team>> {
        let rows = sqlx::query_as::<_, TeamRow>(
            "SELECT * FROM teams ORDER BY created_at DESC LIMIT $1 OFFSET $2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: "Failed to list teams".to_string(),
        })?;

        rows.into_iter().map(|r| r.try_into()).collect()
    }

    #[instrument(skip(self), fields(status = %status, limit = limit, offset = offset), name = "db_list_teams_by_status")]
    async fn list_teams_by_status(
        &self,
        status: TeamStatus,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Team>> {
        let rows = sqlx::query_as::<_, TeamRow>(
            "SELECT * FROM teams WHERE status = $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3",
        )
        .bind(status.as_str())
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: format!("Failed to list teams by status: {}", status),
        })?;

        rows.into_iter().map(|r| r.try_into()).collect()
    }

    #[instrument(skip(self), name = "db_count_teams")]
    async fn count_teams(&self) -> Result<i64> {
        let count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM teams")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| FlowplaneError::Database {
                source: e,
                context: "Failed to count teams".to_string(),
            })?;

        Ok(count)
    }

    #[instrument(skip(self, update), fields(team_id = %id), name = "db_update_team")]
    async fn update_team(&self, id: &TeamId, update: UpdateTeamRequest) -> Result<Team> {
        // Fetch current team
        let current = self
            .get_team_by_id(id)
            .await?
            .ok_or_else(|| FlowplaneError::not_found("Team", id.as_str()))?;

        let display_name = update.display_name.unwrap_or(current.display_name);
        let description = update.description.or(current.description);
        let owner_user_id = update.owner_user_id.or(current.owner_user_id);
        let settings = update.settings.or(current.settings);
        let status = update.status.unwrap_or(current.status);

        let settings_json = settings
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| FlowplaneError::validation(format!("Invalid settings JSON: {}", e)))?;

        let row = sqlx::query_as::<_, TeamRow>(
            "UPDATE teams SET
                display_name = $2,
                description = $3,
                owner_user_id = $4,
                settings = $5,
                status = $6,
                updated_at = $7
            WHERE id = $1
            RETURNING *",
        )
        .bind(id.as_str())
        .bind(&display_name)
        .bind(description.as_deref())
        .bind(owner_user_id.as_ref().map(|id| id.as_str()))
        .bind(settings_json.as_deref())
        .bind(status.as_str())
        .bind(Utc::now())
        .fetch_one(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: format!("Failed to update team: {}", id),
        })?;

        row.try_into()
    }

    #[instrument(skip(self), fields(team_id = %id), name = "db_delete_team")]
    async fn delete_team(&self, id: &TeamId) -> Result<()> {
        let result = sqlx::query("DELETE FROM teams WHERE id = $1")
            .bind(id.as_str())
            .execute(&self.pool)
            .await
            .map_err(|e| FlowplaneError::Database {
                source: e,
                context: format!("Failed to delete team: {}", id),
            })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::not_found("Team", id.as_str()));
        }

        Ok(())
    }

    #[instrument(skip(self), fields(org_id = %org_id), name = "db_list_teams_by_org")]
    async fn list_teams_by_org(&self, org_id: &OrgId) -> Result<Vec<Team>> {
        let rows =
            sqlx::query_as::<_, TeamRow>("SELECT * FROM teams WHERE org_id = $1 ORDER BY name")
                .bind(org_id.as_str())
                .fetch_all(&self.pool)
                .await
                .map_err(|e| FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to list teams by org: {}", org_id),
                })?;

        rows.into_iter().map(|r| r.try_into()).collect()
    }

    #[instrument(skip(self), fields(team_name = %name), name = "db_is_team_name_available")]
    async fn is_name_available(&self, name: &str) -> Result<bool> {
        let count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM teams WHERE name = $1")
            .bind(name)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| FlowplaneError::Database {
                source: e,
                context: format!("Failed to check name availability: {}", name),
            })?;

        Ok(count == 0)
    }

    #[instrument(skip(self), fields(team_count = team_names.len()), name = "db_resolve_team_ids")]
    async fn resolve_team_ids(&self, team_names: &[String]) -> Result<Vec<String>> {
        // Empty input returns empty vec (admin bypass preserved)
        if team_names.is_empty() {
            return Ok(Vec::new());
        }

        // Build dynamic query with IN clause
        // PostgreSQL uses $1, $2, ... for positional params
        let placeholders: Vec<String> = (1..=team_names.len()).map(|i| format!("${}", i)).collect();
        let query =
            format!("SELECT id, name FROM teams WHERE name IN ({})", placeholders.join(", "));

        // Bind all team names to the query
        let mut query_builder = sqlx::query_as::<_, (String, String)>(&query);
        for name in team_names {
            query_builder = query_builder.bind(name);
        }

        let rows =
            query_builder.fetch_all(&self.pool).await.map_err(|e| FlowplaneError::Database {
                source: e,
                context: "Failed to resolve team IDs".to_string(),
            })?;

        // Verify all team names were found
        if rows.len() != team_names.len() {
            let found_names: std::collections::HashSet<_> =
                rows.iter().map(|(_, name)| name.as_str()).collect();
            let missing: Vec<_> =
                team_names.iter().filter(|name| !found_names.contains(name.as_str())).collect();
            return Err(FlowplaneError::validation(format!(
                "Team(s) not found: {}",
                missing.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
            )));
        }

        // Return IDs in the order they were returned (not necessarily the order requested)
        Ok(rows.into_iter().map(|(id, _)| id).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::test_helpers::TestDatabase;

    #[tokio::test]
    async fn test_create_and_get_team() {
        let _db = TestDatabase::new("team_create_get").await;
        let pool = _db.pool.clone();
        let repo = SqlxTeamRepository::new(pool);

        let request = CreateTeamRequest {
            name: "engineering".to_string(),
            display_name: "Engineering Team".to_string(),
            description: Some("Main engineering team".to_string()),
            owner_user_id: None,
            org_id: None,
            settings: Some(serde_json::json!({"foo": "bar"})),
        };

        let created = repo.create_team(request).await.expect("create team");

        assert_eq!(created.name, "engineering");
        assert_eq!(created.display_name, "Engineering Team");
        assert_eq!(created.status, TeamStatus::Active);
        assert!(created.settings.is_some());

        // Get by ID
        let fetched_by_id = repo.get_team_by_id(&created.id).await.expect("get by id");
        assert!(fetched_by_id.is_some());
        assert_eq!(fetched_by_id.as_ref().unwrap().id, created.id);

        // Get by name
        let fetched_by_name = repo.get_team_by_name("engineering").await.expect("get by name");
        assert!(fetched_by_name.is_some());
        assert_eq!(fetched_by_name.unwrap().id, created.id);
    }

    #[tokio::test]
    async fn test_create_team_duplicate_name_fails() {
        let _db = TestDatabase::new("team_dup_name").await;
        let pool = _db.pool.clone();
        let repo = SqlxTeamRepository::new(pool);

        let request = CreateTeamRequest {
            name: "duplicate-test".to_string(),
            display_name: "Duplicate Test Team".to_string(),
            description: None,
            owner_user_id: None,
            org_id: None,
            settings: None,
        };

        repo.create_team(request.clone()).await.expect("first create");

        // Second create with same name should fail
        let result = repo.create_team(request).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_update_team() {
        let _db = TestDatabase::new("team_update").await;
        let pool = _db.pool.clone();
        let repo = SqlxTeamRepository::new(pool);

        let request = CreateTeamRequest {
            name: "devops".to_string(),
            display_name: "DevOps".to_string(),
            description: None,
            owner_user_id: None,
            org_id: None,
            settings: None,
        };

        let created = repo.create_team(request).await.expect("create team");

        let update = UpdateTeamRequest {
            display_name: Some("DevOps Team".to_string()),
            description: Some("Updated description".to_string()),
            owner_user_id: None,
            settings: Some(serde_json::json!({"new": "settings"})),
            status: Some(TeamStatus::Suspended),
        };

        let updated = repo.update_team(&created.id, update).await.expect("update team");

        assert_eq!(updated.id, created.id);
        assert_eq!(updated.name, "devops"); // Name is immutable
        assert_eq!(updated.display_name, "DevOps Team");
        assert_eq!(updated.description.as_deref(), Some("Updated description"));
        assert_eq!(updated.status, TeamStatus::Suspended);
        assert!(updated.settings.is_some());
    }

    #[tokio::test]
    async fn test_list_teams() {
        let _db = TestDatabase::new("team_list").await;
        let pool = _db.pool.clone();
        let repo = SqlxTeamRepository::new(pool);

        // Create multiple teams
        for i in 1..=5 {
            let request = CreateTeamRequest {
                name: format!("team{}", i),
                display_name: format!("Team {}", i),
                description: None,
                owner_user_id: None,
                org_id: None,
                settings: None,
            };
            repo.create_team(request).await.expect("create team");
        }

        // Count includes seed teams from TestDatabase (test-team, team-a, team-b, platform)
        let teams = repo.list_teams(20, 0).await.expect("list teams");
        assert_eq!(teams.len(), 5 + 4); // 5 created + 4 seed

        let count = repo.count_teams().await.expect("count teams");
        assert_eq!(count, 5 + 4);
    }

    #[tokio::test]
    async fn test_list_teams_by_status() {
        let _db = TestDatabase::new("team_list_by_status").await;
        let pool = _db.pool.clone();
        let repo = SqlxTeamRepository::new(pool);

        // Create active team
        let active_request = CreateTeamRequest {
            name: "active-team".to_string(),
            display_name: "Active Team".to_string(),
            description: None,
            owner_user_id: None,
            org_id: None,
            settings: None,
        };
        let active_team = repo.create_team(active_request).await.expect("create active team");

        // Create and suspend another team
        let suspended_request = CreateTeamRequest {
            name: "suspended-team".to_string(),
            display_name: "Suspended Team".to_string(),
            description: None,
            owner_user_id: None,
            org_id: None,
            settings: None,
        };
        let suspended_team =
            repo.create_team(suspended_request).await.expect("create suspended team");

        let update = UpdateTeamRequest {
            display_name: None,
            description: None,
            owner_user_id: None,
            settings: None,
            status: Some(TeamStatus::Suspended),
        };
        repo.update_team(&suspended_team.id, update).await.expect("suspend team");

        // List active teams (includes 4 seed teams + 1 created active)
        let active_teams =
            repo.list_teams_by_status(TeamStatus::Active, 10, 0).await.expect("list active");
        assert_eq!(active_teams.len(), 5); // 1 created + 4 seed
        assert!(active_teams.iter().any(|t| t.id == active_team.id));

        // List suspended teams
        let suspended_teams =
            repo.list_teams_by_status(TeamStatus::Suspended, 10, 0).await.expect("list suspended");
        assert_eq!(suspended_teams.len(), 1);
        assert_eq!(suspended_teams[0].id, suspended_team.id);
    }

    #[tokio::test]
    async fn test_delete_team() {
        let _db = TestDatabase::new("team_delete").await;
        let pool = _db.pool.clone();
        let repo = SqlxTeamRepository::new(pool);

        let request = CreateTeamRequest {
            name: "temp-team".to_string(),
            display_name: "Temporary Team".to_string(),
            description: None,
            owner_user_id: None,
            org_id: None,
            settings: None,
        };

        let created = repo.create_team(request).await.expect("create team");

        repo.delete_team(&created.id).await.expect("delete team");

        let fetched = repo.get_team_by_id(&created.id).await.expect("get deleted team");
        assert!(fetched.is_none());
    }

    #[tokio::test]
    async fn test_is_name_available() {
        let _db = TestDatabase::new("team_name_available").await;
        let pool = _db.pool.clone();
        let repo = SqlxTeamRepository::new(pool);

        assert!(repo.is_name_available("new-team").await.expect("check availability"));

        let request = CreateTeamRequest {
            name: "existing-team".to_string(),
            display_name: "Existing Team".to_string(),
            description: None,
            owner_user_id: None,
            org_id: None,
            settings: None,
        };
        repo.create_team(request).await.expect("create team");

        assert!(!repo.is_name_available("existing-team").await.expect("check availability"));
        assert!(repo.is_name_available("another-team").await.expect("check availability"));
    }

    #[tokio::test]
    async fn test_create_team_allocates_envoy_admin_port() {
        let _db = TestDatabase::new("team_envoy_port").await;
        let pool = _db.pool.clone();
        let repo = SqlxTeamRepository::new(pool);

        let request = CreateTeamRequest {
            name: "first-team".to_string(),
            display_name: "First Team".to_string(),
            description: None,
            owner_user_id: None,
            org_id: None,
            settings: None,
        };

        let created = repo.create_team(request).await.expect("create team");

        // First team should get the base port (9901)
        assert_eq!(created.envoy_admin_port, Some(crate::config::DEFAULT_ENVOY_ADMIN_BASE_PORT));
    }

    #[tokio::test]
    async fn test_create_multiple_teams_unique_ports() {
        let _db = TestDatabase::new("team_unique_ports").await;
        let pool = _db.pool.clone();
        let repo = SqlxTeamRepository::new(pool);

        // Create 3 teams
        let mut ports = Vec::new();
        for i in 1..=3 {
            let request = CreateTeamRequest {
                name: format!("team-{}", i),
                display_name: format!("Team {}", i),
                description: None,
                owner_user_id: None,
                org_id: None,
                settings: None,
            };
            let created = repo.create_team(request).await.expect("create team");
            ports.push(created.envoy_admin_port.expect("port should be allocated"));
        }

        // Verify sequential allocation
        let base = crate::config::DEFAULT_ENVOY_ADMIN_BASE_PORT;
        assert_eq!(ports, vec![base, base + 1, base + 2]);
    }

    #[tokio::test]
    async fn test_resolve_team_ids_empty_input() {
        let _db = TestDatabase::new("team_resolve_empty").await;
        let pool = _db.pool.clone();
        let repo = SqlxTeamRepository::new(pool);

        let result = repo.resolve_team_ids(&[]).await.expect("resolve empty");
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_resolve_team_ids_success() {
        let _db = TestDatabase::new("team_resolve_success").await;
        let pool = _db.pool.clone();
        let repo = SqlxTeamRepository::new(pool);

        // Create teams
        let team1 = repo
            .create_team(CreateTeamRequest {
                name: "alpha".to_string(),
                display_name: "Alpha Team".to_string(),
                description: None,
                owner_user_id: None,
                org_id: None,
                settings: None,
            })
            .await
            .expect("create team alpha");

        let team2 = repo
            .create_team(CreateTeamRequest {
                name: "beta".to_string(),
                display_name: "Beta Team".to_string(),
                description: None,
                owner_user_id: None,
                org_id: None,
                settings: None,
            })
            .await
            .expect("create team beta");

        // Resolve names to IDs
        let names = vec!["alpha".to_string(), "beta".to_string()];
        let ids = repo.resolve_team_ids(&names).await.expect("resolve IDs");

        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&team1.id.as_str().to_string()));
        assert!(ids.contains(&team2.id.as_str().to_string()));
    }

    #[tokio::test]
    async fn test_resolve_team_ids_missing_team() {
        let _db = TestDatabase::new("team_resolve_missing").await;
        let pool = _db.pool.clone();
        let repo = SqlxTeamRepository::new(pool);

        // Create one team
        repo.create_team(CreateTeamRequest {
            name: "exists".to_string(),
            display_name: "Exists Team".to_string(),
            description: None,
            owner_user_id: None,
            org_id: None,
            settings: None,
        })
        .await
        .expect("create team");

        // Try to resolve with a missing team
        let names = vec!["exists".to_string(), "does-not-exist".to_string()];
        let result = repo.resolve_team_ids(&names).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("does-not-exist"));
    }
}
