//! Scope definitions — type-only module.
//!
//! The `scopes` table has been dropped. All scope definitions are now
//! code-only constants in `src/auth/scope_registry.rs`.
//!
//! This module retains only the `ScopeDefinition` type (used by the scopes API handler)
//! and stub implementations so downstream code continues to compile during the
//! transition. The DB-backed `SqlxScopeRepository` and its methods are preserved
//! as stubs that always return empty results — they will be removed in I.5.

use crate::domain::ScopeId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Scope definition with metadata (returned by the scopes API)
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ScopeDefinition {
    /// Unique identifier
    pub id: ScopeId,
    /// The scope value (e.g., "clusters:read")
    pub value: String,
    /// Resource name (e.g., "clusters")
    pub resource: String,
    /// Action name (e.g., "read")
    pub action: String,
    /// Human-readable label for UI
    pub label: String,
    /// Detailed description for UI
    pub description: Option<String>,
    /// Category for UI grouping (e.g., "Clusters")
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

// ---------------------------------------------------------------------------
// Stub types kept for compile-time compatibility — will be removed in I.5
// ---------------------------------------------------------------------------

/// Request to create a new scope (stub — scopes table dropped)
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

/// Request to update an existing scope (stub — scopes table dropped)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateScopeRequest {
    pub label: Option<String>,
    pub description: Option<String>,
    pub category: Option<String>,
    pub visible_in_ui: Option<bool>,
    pub enabled: Option<bool>,
}

/// Repository trait for scope operations (stub — scopes table dropped)
#[async_trait::async_trait]
pub trait ScopeRepository: Send + Sync {
    async fn find_all_enabled(&self) -> crate::errors::Result<Vec<ScopeDefinition>>;
    async fn find_all(&self) -> crate::errors::Result<Vec<ScopeDefinition>>;
    async fn find_ui_visible(&self) -> crate::errors::Result<Vec<ScopeDefinition>>;
    async fn find_by_resource(&self, resource: &str)
        -> crate::errors::Result<Vec<ScopeDefinition>>;
    async fn find_by_value(&self, value: &str) -> crate::errors::Result<Option<ScopeDefinition>>;
    async fn find_by_id(&self, id: &ScopeId) -> crate::errors::Result<Option<ScopeDefinition>>;
    async fn is_valid_scope(&self, value: &str) -> crate::errors::Result<bool>;
    async fn is_valid_resource_action(
        &self,
        resource: &str,
        action: &str,
    ) -> crate::errors::Result<bool>;
    async fn create(&self, request: CreateScopeRequest) -> crate::errors::Result<ScopeDefinition>;
    async fn update(
        &self,
        id: &ScopeId,
        request: UpdateScopeRequest,
    ) -> crate::errors::Result<ScopeDefinition>;
    async fn delete(&self, id: &ScopeId) -> crate::errors::Result<()>;
    async fn get_resources(&self) -> crate::errors::Result<Vec<String>>;
    async fn get_categories(&self) -> crate::errors::Result<Vec<String>>;
}

/// SQLx stub — scopes table dropped, always returns empty/not-found.
pub struct SqlxScopeRepository;

impl SqlxScopeRepository {
    pub fn new(_pool: crate::storage::DbPool) -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl ScopeRepository for SqlxScopeRepository {
    async fn find_all_enabled(&self) -> crate::errors::Result<Vec<ScopeDefinition>> {
        Ok(Vec::new())
    }
    async fn find_all(&self) -> crate::errors::Result<Vec<ScopeDefinition>> {
        Ok(Vec::new())
    }
    async fn find_ui_visible(&self) -> crate::errors::Result<Vec<ScopeDefinition>> {
        Ok(Vec::new())
    }
    async fn find_by_resource(
        &self,
        _resource: &str,
    ) -> crate::errors::Result<Vec<ScopeDefinition>> {
        Ok(Vec::new())
    }
    async fn find_by_value(&self, _value: &str) -> crate::errors::Result<Option<ScopeDefinition>> {
        Ok(None)
    }
    async fn find_by_id(&self, _id: &ScopeId) -> crate::errors::Result<Option<ScopeDefinition>> {
        Ok(None)
    }
    async fn is_valid_scope(&self, _value: &str) -> crate::errors::Result<bool> {
        Ok(false)
    }
    async fn is_valid_resource_action(
        &self,
        _resource: &str,
        _action: &str,
    ) -> crate::errors::Result<bool> {
        Ok(false)
    }
    async fn create(&self, _request: CreateScopeRequest) -> crate::errors::Result<ScopeDefinition> {
        Err(crate::errors::FlowplaneError::internal(
            "scopes table dropped — use VALID_GRANTS constants",
        ))
    }
    async fn update(
        &self,
        id: &ScopeId,
        _request: UpdateScopeRequest,
    ) -> crate::errors::Result<ScopeDefinition> {
        Err(crate::errors::FlowplaneError::not_found("Scope", id.as_str()))
    }
    async fn delete(&self, id: &ScopeId) -> crate::errors::Result<()> {
        Err(crate::errors::FlowplaneError::not_found("Scope", id.as_str()))
    }
    async fn get_resources(&self) -> crate::errors::Result<Vec<String>> {
        Ok(Vec::new())
    }
    async fn get_categories(&self) -> crate::errors::Result<Vec<String>> {
        Ok(Vec::new())
    }
}
