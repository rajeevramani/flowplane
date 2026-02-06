//! Scope repository for authorization scope management
//!
//! This module provides database access for the scope registry,
//! enabling database-driven scope validation and discovery.

use crate::domain::ScopeId;
use crate::errors::{FlowplaneError, Result};
use crate::storage::DbPool;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use utoipa::ToSchema;

/// Database row structure for scopes table
#[derive(Debug, Clone, FromRow)]
struct ScopeRow {
    pub id: String,
    pub value: String,
    pub resource: String,
    pub action: String,
    pub label: String,
    pub description: Option<String>,
    pub category: String,
    pub visible_in_ui: bool,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Scope definition with metadata
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ScopeDefinition {
    /// Unique identifier
    pub id: ScopeId,
    /// The scope value (e.g., "tokens:read")
    pub value: String,
    /// Resource name (e.g., "tokens")
    pub resource: String,
    /// Action name (e.g., "read")
    pub action: String,
    /// Human-readable label for UI
    pub label: String,
    /// Detailed description for UI
    pub description: Option<String>,
    /// Category for UI grouping (e.g., "Tokens")
    pub category: String,
    /// Whether this scope should be shown in UI
    pub visible_in_ui: bool,
    /// Whether this scope is enabled
    pub enabled: bool,
    /// When the scope was created
    pub created_at: DateTime<Utc>,
    /// When the scope was last updated
    pub updated_at: DateTime<Utc>,
}

impl From<ScopeRow> for ScopeDefinition {
    fn from(row: ScopeRow) -> Self {
        Self {
            id: ScopeId::from_string(row.id),
            value: row.value,
            resource: row.resource,
            action: row.action,
            label: row.label,
            description: row.description,
            category: row.category,
            visible_in_ui: row.visible_in_ui,
            enabled: row.enabled,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

/// Request to create a new scope
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateScopeRequest {
    pub value: String,
    pub resource: String,
    pub action: String,
    pub label: String,
    pub description: Option<String>,
    pub category: String,
    pub visible_in_ui: bool,
}

/// Request to update an existing scope
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateScopeRequest {
    pub label: Option<String>,
    pub description: Option<String>,
    pub category: Option<String>,
    pub visible_in_ui: Option<bool>,
    pub enabled: Option<bool>,
}

/// Repository trait for scope operations
#[async_trait]
pub trait ScopeRepository: Send + Sync {
    /// Get all enabled scopes
    async fn find_all_enabled(&self) -> Result<Vec<ScopeDefinition>>;

    /// Get all scopes (including disabled)
    async fn find_all(&self) -> Result<Vec<ScopeDefinition>>;

    /// Get UI-visible scopes only
    async fn find_ui_visible(&self) -> Result<Vec<ScopeDefinition>>;

    /// Get scopes by resource
    async fn find_by_resource(&self, resource: &str) -> Result<Vec<ScopeDefinition>>;

    /// Get a specific scope by value
    async fn find_by_value(&self, value: &str) -> Result<Option<ScopeDefinition>>;

    /// Get a specific scope by ID
    async fn find_by_id(&self, id: &ScopeId) -> Result<Option<ScopeDefinition>>;

    /// Check if a scope value is valid (exists and enabled)
    async fn is_valid_scope(&self, value: &str) -> Result<bool>;

    /// Check if a resource:action combination is valid
    async fn is_valid_resource_action(&self, resource: &str, action: &str) -> Result<bool>;

    /// Create a new scope
    async fn create(&self, request: CreateScopeRequest) -> Result<ScopeDefinition>;

    /// Update an existing scope
    async fn update(&self, id: &ScopeId, request: UpdateScopeRequest) -> Result<ScopeDefinition>;

    /// Delete a scope (soft delete by disabling)
    async fn delete(&self, id: &ScopeId) -> Result<()>;

    /// Get all unique resources that have scopes
    async fn get_resources(&self) -> Result<Vec<String>>;

    /// Get all unique categories
    async fn get_categories(&self) -> Result<Vec<String>>;
}

/// SQLx implementation of ScopeRepository
pub struct SqlxScopeRepository {
    pool: DbPool,
}

impl SqlxScopeRepository {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ScopeRepository for SqlxScopeRepository {
    async fn find_all_enabled(&self) -> Result<Vec<ScopeDefinition>> {
        let rows = sqlx::query_as::<_, ScopeRow>(
            "SELECT * FROM scopes WHERE enabled = TRUE ORDER BY category, resource, action",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: "Failed to fetch enabled scopes".to_string(),
        })?;

        Ok(rows.into_iter().map(ScopeDefinition::from).collect())
    }

    async fn find_all(&self) -> Result<Vec<ScopeDefinition>> {
        let rows = sqlx::query_as::<_, ScopeRow>(
            "SELECT * FROM scopes ORDER BY category, resource, action",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: "Failed to fetch all scopes".to_string(),
        })?;

        Ok(rows.into_iter().map(ScopeDefinition::from).collect())
    }

    async fn find_ui_visible(&self) -> Result<Vec<ScopeDefinition>> {
        let rows = sqlx::query_as::<_, ScopeRow>(
            "SELECT * FROM scopes WHERE enabled = TRUE AND visible_in_ui = TRUE ORDER BY category, resource, action",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: "Failed to fetch UI-visible scopes".to_string(),
        })?;

        Ok(rows.into_iter().map(ScopeDefinition::from).collect())
    }

    async fn find_by_resource(&self, resource: &str) -> Result<Vec<ScopeDefinition>> {
        let rows = sqlx::query_as::<_, ScopeRow>(
            "SELECT * FROM scopes WHERE resource = $1 AND enabled = TRUE ORDER BY action",
        )
        .bind(resource)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: format!("Failed to fetch scopes for resource: {}", resource),
        })?;

        Ok(rows.into_iter().map(ScopeDefinition::from).collect())
    }

    async fn find_by_value(&self, value: &str) -> Result<Option<ScopeDefinition>> {
        let row = sqlx::query_as::<_, ScopeRow>("SELECT * FROM scopes WHERE value = $1")
            .bind(value)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| FlowplaneError::Database {
                source: e,
                context: format!("Failed to fetch scope by value: {}", value),
            })?;

        Ok(row.map(ScopeDefinition::from))
    }

    async fn find_by_id(&self, id: &ScopeId) -> Result<Option<ScopeDefinition>> {
        let row = sqlx::query_as::<_, ScopeRow>("SELECT * FROM scopes WHERE id = $1")
            .bind(id.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| FlowplaneError::Database {
                source: e,
                context: format!("Failed to fetch scope by ID: {}", id),
            })?;

        Ok(row.map(ScopeDefinition::from))
    }

    async fn is_valid_scope(&self, value: &str) -> Result<bool> {
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM scopes WHERE value = $1 AND enabled = TRUE",
        )
        .bind(value)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: format!("Failed to validate scope: {}", value),
        })?;

        Ok(count > 0)
    }

    async fn is_valid_resource_action(&self, resource: &str, action: &str) -> Result<bool> {
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM scopes WHERE resource = $1 AND action = $2 AND enabled = TRUE",
        )
        .bind(resource)
        .bind(action)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: format!("Failed to validate resource:action: {}:{}", resource, action),
        })?;

        Ok(count > 0)
    }

    async fn create(&self, request: CreateScopeRequest) -> Result<ScopeDefinition> {
        let id = ScopeId::new();
        let now = Utc::now();

        let row = sqlx::query_as::<_, ScopeRow>(
            "INSERT INTO scopes (
                id, value, resource, action, label, description, category, visible_in_ui, enabled, created_at, updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, TRUE, $9, $10)
            RETURNING *",
        )
        .bind(id.as_str())
        .bind(&request.value)
        .bind(&request.resource)
        .bind(&request.action)
        .bind(&request.label)
        .bind(request.description.as_deref())
        .bind(&request.category)
        .bind(request.visible_in_ui)
        .bind(now)
        .bind(now)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: format!("Failed to create scope: {}", request.value),
        })?;

        Ok(ScopeDefinition::from(row))
    }

    async fn update(&self, id: &ScopeId, request: UpdateScopeRequest) -> Result<ScopeDefinition> {
        // Fetch current scope
        let current = self
            .find_by_id(id)
            .await?
            .ok_or_else(|| FlowplaneError::not_found("Scope", id.as_str()))?;

        let label = request.label.unwrap_or(current.label);
        let description = request.description.or(current.description);
        let category = request.category.unwrap_or(current.category);
        let visible_in_ui = request.visible_in_ui.unwrap_or(current.visible_in_ui);
        let enabled = request.enabled.unwrap_or(current.enabled);

        let row = sqlx::query_as::<_, ScopeRow>(
            "UPDATE scopes SET
                label = $2,
                description = $3,
                category = $4,
                visible_in_ui = $5,
                enabled = $6,
                updated_at = $7
            WHERE id = $1
            RETURNING *",
        )
        .bind(id.as_str())
        .bind(&label)
        .bind(description.as_deref())
        .bind(&category)
        .bind(visible_in_ui)
        .bind(enabled)
        .bind(Utc::now())
        .fetch_one(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: format!("Failed to update scope: {}", id),
        })?;

        Ok(ScopeDefinition::from(row))
    }

    async fn delete(&self, id: &ScopeId) -> Result<()> {
        // Soft delete by disabling
        let result =
            sqlx::query("UPDATE scopes SET enabled = FALSE, updated_at = $2 WHERE id = $1")
                .bind(id.as_str())
                .bind(Utc::now())
                .execute(&self.pool)
                .await
                .map_err(|e| FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to delete scope: {}", id),
                })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::not_found("Scope", id.as_str()));
        }

        Ok(())
    }

    async fn get_resources(&self) -> Result<Vec<String>> {
        let resources = sqlx::query_scalar::<_, String>(
            "SELECT DISTINCT resource FROM scopes WHERE enabled = TRUE ORDER BY resource",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: "Failed to fetch scope resources".to_string(),
        })?;

        Ok(resources)
    }

    async fn get_categories(&self) -> Result<Vec<String>> {
        let categories = sqlx::query_scalar::<_, String>(
            "SELECT DISTINCT category FROM scopes WHERE enabled = TRUE ORDER BY category",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: "Failed to fetch scope categories".to_string(),
        })?;

        Ok(categories)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::test_helpers::TestDatabase;

    #[tokio::test]
    async fn test_find_all_enabled_returns_seeded_scopes() {
        let _db = TestDatabase::new("scope_find_all_enabled").await;
        let pool = _db.pool.clone();
        let repo = SqlxScopeRepository::new(pool);

        let scopes = repo.find_all_enabled().await.expect("find_all_enabled");

        // Should have seeded scopes from migration
        assert!(!scopes.is_empty());

        // Check that tokens:read exists
        let tokens_read = scopes.iter().find(|s| s.value == "tokens:read");
        assert!(tokens_read.is_some());
        assert_eq!(tokens_read.unwrap().resource, "tokens");
        assert_eq!(tokens_read.unwrap().action, "read");
    }

    #[tokio::test]
    async fn test_find_ui_visible_excludes_admin() {
        let _db = TestDatabase::new("scope_find_ui_visible").await;
        let pool = _db.pool.clone();
        let repo = SqlxScopeRepository::new(pool);

        let scopes = repo.find_ui_visible().await.expect("find_ui_visible");

        // admin:all should not be visible
        let admin_scope = scopes.iter().find(|s| s.value == "admin:all");
        assert!(admin_scope.is_none());

        // tokens:read should be visible
        let tokens_read = scopes.iter().find(|s| s.value == "tokens:read");
        assert!(tokens_read.is_some());
    }

    #[tokio::test]
    async fn test_find_by_value() {
        let _db = TestDatabase::new("scope_find_by_value").await;
        let pool = _db.pool.clone();
        let repo = SqlxScopeRepository::new(pool);

        let scope = repo.find_by_value("clusters:write").await.expect("find_by_value");

        assert!(scope.is_some());
        let s = scope.unwrap();
        assert_eq!(s.resource, "clusters");
        assert_eq!(s.action, "write");
        assert_eq!(s.category, "Clusters");
    }

    #[tokio::test]
    async fn test_find_by_value_not_found() {
        let _db = TestDatabase::new("scope_find_by_value_nf").await;
        let pool = _db.pool.clone();
        let repo = SqlxScopeRepository::new(pool);

        let scope = repo.find_by_value("nonexistent:scope").await.expect("find_by_value");

        assert!(scope.is_none());
    }

    #[tokio::test]
    async fn test_is_valid_scope() {
        let _db = TestDatabase::new("scope_is_valid").await;
        let pool = _db.pool.clone();
        let repo = SqlxScopeRepository::new(pool);

        // Valid scopes
        assert!(repo.is_valid_scope("tokens:read").await.expect("valid"));
        assert!(repo.is_valid_scope("clusters:write").await.expect("valid"));
        assert!(repo.is_valid_scope("admin:all").await.expect("valid"));

        // Invalid scope
        assert!(!repo.is_valid_scope("foo:bar").await.expect("invalid"));
    }

    #[tokio::test]
    async fn test_is_valid_resource_action() {
        let _db = TestDatabase::new("scope_valid_resource_action").await;
        let pool = _db.pool.clone();
        let repo = SqlxScopeRepository::new(pool);

        // Valid combinations
        assert!(repo.is_valid_resource_action("tokens", "read").await.expect("valid"));
        assert!(repo.is_valid_resource_action("clusters", "delete").await.expect("valid"));

        // Invalid combinations
        assert!(!repo.is_valid_resource_action("foo", "bar").await.expect("invalid"));
        assert!(!repo.is_valid_resource_action("tokens", "execute").await.expect("invalid"));
    }

    #[tokio::test]
    async fn test_find_by_resource() {
        let _db = TestDatabase::new("scope_find_by_resource").await;
        let pool = _db.pool.clone();
        let repo = SqlxScopeRepository::new(pool);

        let scopes = repo.find_by_resource("tokens").await.expect("find_by_resource");

        assert_eq!(scopes.len(), 3); // read, write, delete
        assert!(scopes.iter().all(|s| s.resource == "tokens"));
    }

    #[tokio::test]
    async fn test_create_scope() {
        let _db = TestDatabase::new("scope_create").await;
        let pool = _db.pool.clone();
        let repo = SqlxScopeRepository::new(pool);

        let request = CreateScopeRequest {
            value: "custom:action".to_string(),
            resource: "custom".to_string(),
            action: "action".to_string(),
            label: "Custom Action".to_string(),
            description: Some("A custom scope".to_string()),
            category: "Custom".to_string(),
            visible_in_ui: true,
        };

        let created = repo.create(request).await.expect("create scope");

        assert_eq!(created.value, "custom:action");
        assert_eq!(created.resource, "custom");
        assert_eq!(created.action, "action");
        assert!(created.enabled);

        // Verify it's retrievable
        let fetched = repo.find_by_value("custom:action").await.expect("fetch created");
        assert!(fetched.is_some());
    }

    #[tokio::test]
    async fn test_update_scope() {
        let _db = TestDatabase::new("scope_update").await;
        let pool = _db.pool.clone();
        let repo = SqlxScopeRepository::new(pool);

        // Get an existing scope
        let scope =
            repo.find_by_value("tokens:read").await.expect("find scope").expect("scope exists");

        let update = UpdateScopeRequest {
            label: Some("View Tokens".to_string()),
            description: Some("Updated description".to_string()),
            category: None,
            visible_in_ui: None,
            enabled: None,
        };

        let updated = repo.update(&scope.id, update).await.expect("update scope");

        assert_eq!(updated.label, "View Tokens");
        assert_eq!(updated.description.as_deref(), Some("Updated description"));
        assert_eq!(updated.category, "Tokens"); // Unchanged
    }

    #[tokio::test]
    async fn test_delete_scope_soft_deletes() {
        let _db = TestDatabase::new("scope_delete").await;
        let pool = _db.pool.clone();
        let repo = SqlxScopeRepository::new(pool);

        // Get an existing scope
        let scope =
            repo.find_by_value("tokens:read").await.expect("find scope").expect("scope exists");

        repo.delete(&scope.id).await.expect("delete scope");

        // Should no longer be valid
        assert!(!repo.is_valid_scope("tokens:read").await.expect("invalid"));

        // But should still exist in DB (soft delete)
        let fetched = repo.find_by_id(&scope.id).await.expect("fetch");
        assert!(fetched.is_some());
        assert!(!fetched.unwrap().enabled);
    }

    #[tokio::test]
    async fn test_get_resources() {
        let _db = TestDatabase::new("scope_get_resources").await;
        let pool = _db.pool.clone();
        let repo = SqlxScopeRepository::new(pool);

        let resources = repo.get_resources().await.expect("get_resources");

        assert!(resources.contains(&"tokens".to_string()));
        assert!(resources.contains(&"clusters".to_string()));
        assert!(resources.contains(&"routes".to_string()));
    }

    #[tokio::test]
    async fn test_get_categories() {
        let _db = TestDatabase::new("scope_get_categories").await;
        let pool = _db.pool.clone();
        let repo = SqlxScopeRepository::new(pool);

        let categories = repo.get_categories().await.expect("get_categories");

        assert!(categories.contains(&"Tokens".to_string()));
        assert!(categories.contains(&"Clusters".to_string()));
        assert!(categories.contains(&"Admin".to_string()));
    }
}
