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
use tracing::instrument;
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
    pub user_id: Option<String>,
    pub user_email: Option<String>,
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
    pub failed_attempts: i64,
    pub locked_until: Option<chrono::DateTime<chrono::Utc>>,
    pub status: String,
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
    pub user_id: Option<String>,
    pub user_email: Option<String>,
    // Scope field (nullable because of LEFT JOIN)
    pub scope: Option<String>,
}

#[async_trait]
pub trait TokenRepository: Send + Sync {
    async fn create_token(&self, token: NewPersonalAccessToken) -> Result<PersonalAccessToken>;
    async fn list_tokens(
        &self,
        limit: i64,
        offset: i64,
        created_by: Option<&str>,
    ) -> Result<Vec<PersonalAccessToken>>;
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
    async fn record_failed_setup_token_attempt(&self, id: &str) -> Result<()>;
    async fn revoke_setup_token(&self, id: &str) -> Result<()>;

    // CSRF token operations
    async fn store_csrf_token(&self, token_id: &TokenId, csrf_token: &str) -> Result<()>;
    async fn get_csrf_token(&self, token_id: &TokenId) -> Result<Option<String>>;
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
            user_id: row.user_id.map(crate::domain::UserId::from_string),
            user_email: row.user_email,
        })
    }

    #[instrument(skip(self), fields(token_id = %id), name = "db_scopes_for_token")]
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
    #[instrument(skip(self, token), fields(token_name = %token.name, token_id = %token.id), name = "db_create_token")]
    async fn create_token(&self, token: NewPersonalAccessToken) -> Result<PersonalAccessToken> {
        let mut tx = self.pool.begin().await.map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to begin transaction for token creation".to_string(),
        })?;

        sqlx::query(
            "INSERT INTO personal_access_tokens (id, name, token_hash, description, status, expires_at, created_by, is_setup_token, max_usage_count, usage_count, failed_attempts, locked_until, user_id, user_email, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)"
        )
        .bind(&token.id)
        .bind(&token.name)
        .bind(&token.hashed_secret)
        .bind(token.description.as_ref())
        .bind(token.status.as_str())
        .bind(token.expires_at)
        .bind(token.created_by.as_ref())
        .bind(token.is_setup_token)
        .bind(token.max_usage_count)
        .bind(token.usage_count)
        .bind(token.failed_attempts)
        .bind(token.locked_until)
        .bind(token.user_id.as_ref().map(|id| id.to_string()))
        .bind(token.user_email.as_ref())
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

    #[instrument(skip(self), fields(limit = limit, offset = offset, created_by = ?created_by), name = "db_list_tokens")]
    async fn list_tokens(
        &self,
        limit: i64,
        offset: i64,
        created_by: Option<&str>,
    ) -> Result<Vec<PersonalAccessToken>> {
        let limit = limit.clamp(1, 1000);

        // Optimized query using subquery + LEFT JOIN to fetch tokens and scopes in a single query
        // The subquery ensures we LIMIT distinct tokens first, then join with scopes
        // This eliminates the N+1 pattern where we previously made 1 + 2N queries

        // Build query with optional created_by filter
        let rows: Vec<TokenWithScopeRow> = if let Some(creator) = created_by {
            let sql = r#"
                SELECT
                    t.id, t.name, t.description, t.token_hash, t.status,
                    t.expires_at, t.last_used_at, t.created_by, t.created_at, t.updated_at,
                    t.user_id, t.user_email,
                    s.scope
                FROM (
                    SELECT * FROM personal_access_tokens
                    WHERE created_by = $3
                    ORDER BY created_at DESC
                    LIMIT $1 OFFSET $2
                ) t
                LEFT JOIN token_scopes s ON t.id = s.token_id
                ORDER BY t.created_at DESC, s.scope ASC
            "#
            .to_string();

            let result = sqlx::query_as(&sql)
                .bind(limit)
                .bind(offset)
                .bind(creator)
                .fetch_all(&self.pool)
                .await
                .map_err(|err| FlowplaneError::Database {
                    source: err,
                    context: "Failed to list personal access tokens with scopes".to_string(),
                })?;

            result
        } else {
            let sql = r#"
                SELECT
                    t.id, t.name, t.description, t.token_hash, t.status,
                    t.expires_at, t.last_used_at, t.created_by, t.created_at, t.updated_at,
                    t.user_id, t.user_email,
                    s.scope
                FROM (
                    SELECT * FROM personal_access_tokens
                    ORDER BY created_at DESC
                    LIMIT $1 OFFSET $2
                ) t
                LEFT JOIN token_scopes s ON t.id = s.token_id
                ORDER BY t.created_at DESC, s.scope ASC
            "#
            .to_string();

            let result =
                sqlx::query_as(&sql).bind(limit).bind(offset).fetch_all(&self.pool).await.map_err(
                    |err| FlowplaneError::Database {
                        source: err,
                        context: "Failed to list personal access tokens with scopes".to_string(),
                    },
                )?;

            result
        };

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
                user_id: row.user_id,
                user_email: row.user_email,
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

    #[instrument(skip(self), fields(token_id = %id), name = "db_get_token")]
    async fn get_token(&self, id: &TokenId) -> Result<PersonalAccessToken> {
        let row: PersonalAccessTokenRow = sqlx::query_as(
            "SELECT id, name, description, token_hash, status, expires_at, last_used_at, created_by, created_at, updated_at, user_id, user_email FROM personal_access_tokens WHERE id = $1"
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

    #[instrument(skip(self, update), fields(token_id = %id), name = "db_update_token_metadata")]
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
            "SELECT id, name, description, token_hash, status, expires_at, last_used_at, created_by, created_at, updated_at, user_id, user_email FROM personal_access_tokens WHERE id = $1"
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

    #[instrument(skip(self, hashed_secret), fields(token_id = %id), name = "db_rotate_token_secret")]
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

    #[instrument(skip(self), fields(token_id = %id), name = "db_update_token_last_used")]
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

    #[instrument(skip(self), fields(token_id = %id), name = "db_find_active_token_for_auth")]
    async fn find_active_for_auth(
        &self,
        id: &TokenId,
    ) -> Result<Option<(PersonalAccessToken, String)>> {
        let row: Option<PersonalAccessTokenRow> = sqlx::query_as(
            "SELECT id, name, description, token_hash, status, expires_at, last_used_at, created_by, created_at, updated_at, user_id, user_email FROM personal_access_tokens WHERE id = $1"
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

    #[instrument(skip(self), name = "db_count_tokens")]
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

    #[instrument(skip(self), name = "db_count_active_tokens")]
    async fn count_active_tokens(&self) -> Result<i64> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM personal_access_tokens WHERE status = 'active' AND is_setup_token = FALSE",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to count active personal access tokens".to_string(),
        })?;
        Ok(count)
    }

    #[instrument(skip(self), fields(token_id = %id), name = "db_get_setup_token_for_validation")]
    async fn get_setup_token_for_validation(&self, id: &str) -> Result<SetupTokenValidationData> {
        #[derive(Debug, Clone, FromRow)]
        struct SetupTokenRow {
            pub token_hash: String,
            pub is_setup_token: bool,
            pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
            pub max_usage_count: Option<i64>,
            pub usage_count: i64,
            pub failed_attempts: i64,
            pub locked_until: Option<chrono::DateTime<chrono::Utc>>,
            pub status: String,
        }

        let row: SetupTokenRow = sqlx::query_as(
            "SELECT token_hash, is_setup_token, expires_at, max_usage_count, usage_count, failed_attempts, locked_until, status
             FROM personal_access_tokens
             WHERE id = $1",
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
            failed_attempts: row.failed_attempts,
            locked_until: row.locked_until,
            status: row.status,
        })
    }

    #[instrument(skip(self), fields(token_id = %id), name = "db_increment_setup_token_usage")]
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

    #[instrument(skip(self), fields(token_id = %id), name = "db_record_failed_setup_token_attempt")]
    async fn record_failed_setup_token_attempt(&self, id: &str) -> Result<()> {
        // Increment failed_attempts and lock token if failed_attempts >= 5
        // Lock for 15 minutes
        let result = sqlx::query(
            "UPDATE personal_access_tokens
             SET failed_attempts = failed_attempts + 1,
                 locked_until = CASE
                     WHEN failed_attempts + 1 >= 5 THEN datetime(CURRENT_TIMESTAMP, '+15 minutes')
                     ELSE locked_until
                 END,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = $1 AND is_setup_token = TRUE",
        )
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to record failed setup token attempt".to_string(),
        })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::not_found("setup_token", id));
        }

        Ok(())
    }

    #[instrument(skip(self), fields(token_id = %id), name = "db_revoke_setup_token")]
    async fn revoke_setup_token(&self, id: &str) -> Result<()> {
        let result = sqlx::query(
            "UPDATE personal_access_tokens
             SET status = 'revoked',
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = $1 AND is_setup_token = TRUE",
        )
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to revoke setup token".to_string(),
        })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::not_found("setup_token", id));
        }

        Ok(())
    }

    #[instrument(skip(self, csrf_token), fields(token_id = %token_id), name = "db_store_csrf_token")]
    async fn store_csrf_token(&self, token_id: &TokenId, csrf_token: &str) -> Result<()> {
        let result = sqlx::query(
            "UPDATE personal_access_tokens
             SET csrf_token = $1,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = $2",
        )
        .bind(csrf_token)
        .bind(token_id)
        .execute(&self.pool)
        .await
        .map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to store CSRF token".to_string(),
        })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::not_found("token", token_id.as_str()));
        }

        Ok(())
    }

    #[instrument(skip(self), fields(token_id = %token_id), name = "db_get_csrf_token")]
    async fn get_csrf_token(&self, token_id: &TokenId) -> Result<Option<String>> {
        let row: Option<(Option<String>,)> =
            sqlx::query_as("SELECT csrf_token FROM personal_access_tokens WHERE id = $1")
                .bind(token_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|err| FlowplaneError::Database {
                    source: err,
                    context: "Failed to get CSRF token".to_string(),
                })?;

        Ok(row.and_then(|(csrf,)| csrf))
    }
}
