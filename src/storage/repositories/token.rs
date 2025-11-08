//! Token repository for authentication token management
//!
//! This module provides CRUD operations for personal access tokens, including
//! token creation, rotation, and authentication lookups.

use crate::auth::models::{
    NewPersonalAccessToken, PersonalAccessToken, TokenStatus, UpdatePersonalAccessToken,
};
use crate::domain::TokenId;
use crate::errors::{FlowplaneError, Result};
use crate::storage::DbPool;
use async_trait::async_trait;
use sqlx::FromRow;
use std::str::FromStr;
use uuid::Uuid;

#[derive(Debug, Clone, FromRow)]
struct PersonalAccessTokenRow {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub token_hash: String,
    pub status: String,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_used_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_by: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, FromRow)]
struct TokenScopeRow {
    pub scope: String,
}

/// Setup token data for validation
#[derive(Debug, Clone)]
pub struct SetupTokenValidationData {
    pub token_hash: String,
    pub is_setup_token: bool,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub max_usage_count: Option<i64>,
    pub usage_count: i64,
}

/// Combined row for batch fetching tokens with scopes via LEFT JOIN
#[derive(Debug, Clone, FromRow)]
struct TokenWithScopeRow {
    // Token fields
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub token_hash: String,
    pub status: String,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_used_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_by: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    // Scope field (nullable because of LEFT JOIN)
    pub scope: Option<String>,
}

#[async_trait]
pub trait TokenRepository: Send + Sync {
    async fn create_token(&self, token: NewPersonalAccessToken) -> Result<PersonalAccessToken>;
    async fn list_tokens(&self, limit: i64, offset: i64) -> Result<Vec<PersonalAccessToken>>;
    async fn get_token(&self, id: &TokenId) -> Result<PersonalAccessToken>;
    async fn update_metadata(
        &self,
        id: &TokenId,
        update: UpdatePersonalAccessToken,
    ) -> Result<PersonalAccessToken>;
    async fn rotate_secret(&self, id: &TokenId, hashed_secret: String) -> Result<()>;
    async fn update_last_used(
        &self,
        id: &TokenId,
        when: chrono::DateTime<chrono::Utc>,
    ) -> Result<()>;
    async fn find_active_for_auth(
        &self,
        id: &TokenId,
    ) -> Result<Option<(PersonalAccessToken, String)>>;
    async fn count_tokens(&self) -> Result<i64>;
    async fn count_active_tokens(&self) -> Result<i64>;
    async fn get_setup_token_for_validation(&self, id: &str) -> Result<SetupTokenValidationData>;
    async fn increment_setup_token_usage(&self, id: &str) -> Result<()>;
}

#[derive(Debug, Clone)]
pub struct SqlxTokenRepository {
    pool: DbPool,
}

impl SqlxTokenRepository {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    fn to_model(
        &self,
        row: PersonalAccessTokenRow,
        scopes: Vec<String>,
    ) -> Result<PersonalAccessToken> {
        let status = TokenStatus::from_str(&row.status).map_err(|_| {
            FlowplaneError::validation(format!(
                "Unknown token status '{}' for token {}",
                row.status, row.id
            ))
        })?;

        Ok(PersonalAccessToken {
            id: TokenId::from_string(row.id),
            name: row.name,
            description: row.description,
            status,
            expires_at: row.expires_at,
            last_used_at: row.last_used_at,
            created_by: row.created_by,
            created_at: row.created_at,
            updated_at: row.updated_at,
            scopes,
        })
    }

    async fn scopes_for_token(&self, id: &TokenId) -> Result<Vec<String>> {
        let rows: Vec<TokenScopeRow> =
            sqlx::query_as("SELECT scope FROM token_scopes WHERE token_id = $1 ORDER BY scope")
                .bind(id)
                .fetch_all(&self.pool)
                .await
                .map_err(|err| FlowplaneError::Database {
                    source: err,
                    context: "Failed to fetch token scopes".to_string(),
                })?;

        Ok(rows.into_iter().map(|row| row.scope).collect())
    }
}

#[async_trait]
impl TokenRepository for SqlxTokenRepository {
    async fn create_token(&self, token: NewPersonalAccessToken) -> Result<PersonalAccessToken> {
        let mut tx = self.pool.begin().await.map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to begin transaction for token creation".to_string(),
        })?;

        sqlx::query(
            "INSERT INTO personal_access_tokens (id, name, token_hash, description, status, expires_at, created_by, created_at, updated_at)              VALUES ($1, $2, $3, $4, $5, $6, $7, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)"
        )
        .bind(&token.id)
        .bind(&token.name)
        .bind(&token.hashed_secret)
        .bind(token.description.as_ref())
        .bind(token.status.as_str())
        .bind(token.expires_at)
        .bind(token.created_by.as_ref())
        .execute(&mut *tx)
        .await
        .map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to insert personal access token".to_string(),
        })?;

        for scope in &token.scopes {
            sqlx::query(
                "INSERT INTO token_scopes (id, token_id, scope, created_at) VALUES ($1, $2, $3, CURRENT_TIMESTAMP)"
            )
            .bind(Uuid::new_v4().to_string())
            .bind(&token.id)
            .bind(scope)
            .execute(&mut *tx)
            .await
            .map_err(|err| FlowplaneError::Database {
                source: err,
                context: "Failed to insert token scope".to_string(),
            })?;
        }

        tx.commit().await.map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to commit token creation".to_string(),
        })?;

        self.get_token(&token.id).await
    }

    async fn list_tokens(&self, limit: i64, offset: i64) -> Result<Vec<PersonalAccessToken>> {
        let limit = limit.clamp(1, 1000);

        // Optimized query using subquery + LEFT JOIN to fetch tokens and scopes in a single query
        // The subquery ensures we LIMIT distinct tokens first, then join with scopes
        // This eliminates the N+1 pattern where we previously made 1 + 2N queries
        let rows: Vec<TokenWithScopeRow> = sqlx::query_as(
            r#"
            SELECT
                t.id, t.name, t.description, t.token_hash, t.status,
                t.expires_at, t.last_used_at, t.created_by, t.created_at, t.updated_at,
                s.scope
            FROM (
                SELECT * FROM personal_access_tokens
                ORDER BY created_at DESC
                LIMIT $1 OFFSET $2
            ) t
            LEFT JOIN token_scopes s ON t.id = s.token_id
            ORDER BY t.created_at DESC, s.scope ASC
            "#,
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to list personal access tokens with scopes".to_string(),
        })?;

        // Group rows by token ID and aggregate scopes in memory
        use std::collections::HashMap;
        let mut token_map: HashMap<String, (PersonalAccessTokenRow, Vec<String>)> = HashMap::new();

        for row in rows {
            let token_row = PersonalAccessTokenRow {
                id: row.id.clone(),
                name: row.name,
                description: row.description,
                token_hash: row.token_hash,
                status: row.status,
                expires_at: row.expires_at,
                last_used_at: row.last_used_at,
                created_by: row.created_by,
                created_at: row.created_at,
                updated_at: row.updated_at,
            };

            let entry = token_map.entry(row.id).or_insert((token_row, Vec::new()));
            if let Some(scope) = row.scope {
                entry.1.push(scope);
            }
        }

        // Convert aggregated data into PersonalAccessToken models
        let mut tokens: Vec<PersonalAccessToken> = token_map
            .into_values()
            .map(|(token_row, scopes)| self.to_model(token_row, scopes))
            .collect::<Result<Vec<_>>>()?;

        // Sort by created_at DESC to maintain original ordering
        tokens.sort_by(|a, b| b.created_at.cmp(&a.created_at));

        Ok(tokens)
    }

    async fn get_token(&self, id: &TokenId) -> Result<PersonalAccessToken> {
        let row: PersonalAccessTokenRow = sqlx::query_as(
            "SELECT id, name, description, token_hash, status, expires_at, last_used_at, created_by, created_at, updated_at              FROM personal_access_tokens WHERE id = $1"
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to fetch personal access token".to_string(),
        })?
        .ok_or_else(|| FlowplaneError::not_found_msg(format!("Token '{}' not found", id)))?;

        let scopes = self.scopes_for_token(id).await?;
        self.to_model(row, scopes)
    }

    async fn update_metadata(
        &self,
        id: &TokenId,
        update: UpdatePersonalAccessToken,
    ) -> Result<PersonalAccessToken> {
        let mut tx = self.pool.begin().await.map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to begin transaction for token update".to_string(),
        })?;

        let existing: PersonalAccessTokenRow = sqlx::query_as(
            "SELECT id, name, description, token_hash, status, expires_at, last_used_at, created_by, created_at, updated_at              FROM personal_access_tokens WHERE id = $1"
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to fetch personal access token".to_string(),
        })?
        .ok_or_else(|| FlowplaneError::not_found_msg(format!("Token '{}' not found", id)))?;

        let base_status = TokenStatus::from_str(&existing.status).map_err(|_| {
            FlowplaneError::validation(format!(
                "Unknown token status '{}' for token {}",
                existing.status, existing.id
            ))
        })?;

        let name = update.name.unwrap_or(existing.name.clone());
        let description = update.description.or(existing.description.clone());
        let status = update.status.unwrap_or(base_status);
        let expires_at = update.expires_at.unwrap_or(existing.expires_at);

        sqlx::query(
            "UPDATE personal_access_tokens SET name = $1, description = $2, status = $3, expires_at = $4, updated_at = CURRENT_TIMESTAMP WHERE id = $5"
        )
        .bind(&name)
        .bind(description.as_ref())
        .bind(status.as_str())
        .bind(expires_at)
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to update personal access token".to_string(),
        })?;

        if let Some(scopes) = update.scopes {
            sqlx::query("DELETE FROM token_scopes WHERE token_id = $1")
                .bind(id)
                .execute(&mut *tx)
                .await
                .map_err(|err| FlowplaneError::Database {
                    source: err,
                    context: "Failed to delete token scopes".to_string(),
                })?;

            for scope in scopes {
                sqlx::query(
                    "INSERT INTO token_scopes (id, token_id, scope, created_at) VALUES ($1, $2, $3, CURRENT_TIMESTAMP)"
                )
                .bind(Uuid::new_v4().to_string())
                .bind(id)
                .bind(&scope)
                .execute(&mut *tx)
                .await
                .map_err(|err| FlowplaneError::Database {
                    source: err,
                    context: "Failed to insert token scope".to_string(),
                })?;
            }
        }

        tx.commit().await.map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to commit token update".to_string(),
        })?;

        self.get_token(id).await
    }

    async fn rotate_secret(&self, id: &TokenId, hashed_secret: String) -> Result<()> {
        sqlx::query(
            "UPDATE personal_access_tokens SET token_hash = $1, updated_at = CURRENT_TIMESTAMP WHERE id = $2"
        )
        .bind(&hashed_secret)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to rotate token secret".to_string(),
        })?;
        Ok(())
    }

    async fn update_last_used(
        &self,
        id: &TokenId,
        when: chrono::DateTime<chrono::Utc>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE personal_access_tokens SET last_used_at = $1, updated_at = CURRENT_TIMESTAMP WHERE id = $2"
        )
        .bind(when)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to update token last_used_at".to_string(),
        })?;
        Ok(())
    }

    async fn find_active_for_auth(
        &self,
        id: &TokenId,
    ) -> Result<Option<(PersonalAccessToken, String)>> {
        let row: Option<PersonalAccessTokenRow> = sqlx::query_as(
            "SELECT id, name, description, token_hash, status, expires_at, last_used_at, created_by, created_at, updated_at              FROM personal_access_tokens WHERE id = $1"
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to fetch personal access token".to_string(),
        })?;

        let Some(row) = row else {
            return Ok(None);
        };

        let hashed = row.token_hash.clone();
        let token_id = TokenId::from_string(row.id.clone());
        let scopes = self.scopes_for_token(&token_id).await?;
        let model = self.to_model(row, scopes)?;
        Ok(Some((model, hashed)))
    }

    async fn count_tokens(&self) -> Result<i64> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM personal_access_tokens")
            .fetch_one(&self.pool)
            .await
            .map_err(|err| FlowplaneError::Database {
                source: err,
                context: "Failed to count personal access tokens".to_string(),
            })?;
        Ok(count)
    }

    async fn count_active_tokens(&self) -> Result<i64> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM personal_access_tokens WHERE status = 'active'",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to count active personal access tokens".to_string(),
        })?;
        Ok(count)
    }

    async fn get_setup_token_for_validation(&self, id: &str) -> Result<SetupTokenValidationData> {
        #[derive(Debug, Clone, FromRow)]
        struct SetupTokenRow {
            pub token_hash: String,
            pub is_setup_token: bool,
            pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
            pub max_usage_count: Option<i64>,
            pub usage_count: i64,
        }

        let row: SetupTokenRow = sqlx::query_as(
            "SELECT token_hash, is_setup_token, expires_at, max_usage_count, usage_count
             FROM personal_access_tokens
             WHERE id = $1 AND status = 'active'",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await
        .map_err(|err| match err {
            sqlx::Error::RowNotFound => FlowplaneError::not_found("setup_token", id),
            _ => FlowplaneError::Database {
                source: err,
                context: "Failed to fetch setup token for validation".to_string(),
            },
        })?;

        Ok(SetupTokenValidationData {
            token_hash: row.token_hash,
            is_setup_token: row.is_setup_token,
            expires_at: row.expires_at,
            max_usage_count: row.max_usage_count,
            usage_count: row.usage_count,
        })
    }

    async fn increment_setup_token_usage(&self, id: &str) -> Result<()> {
        let result = sqlx::query(
            "UPDATE personal_access_tokens
             SET usage_count = usage_count + 1,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = $1 AND is_setup_token = TRUE",
        )
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to increment setup token usage count".to_string(),
        })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::not_found("setup_token", id));
        }

        Ok(())
    }
}
