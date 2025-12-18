//! Proxy certificate repository for mTLS certificate tracking.
//!
//! This module provides CRUD operations for proxy certificates, enabling:
//! - Audit trail of certificate issuance
//! - Expiry tracking for renewal notifications
//! - Certificate revocation support

use crate::domain::{ProxyCertificateId, TeamId, UserId};
use crate::errors::{FlowplaneError, Result};
use crate::storage::DbPool;
use async_trait::async_trait;
use chrono::{DateTime, NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use tracing::instrument;
use utoipa::ToSchema;

/// Parse a timestamp string that may be in RFC 3339 format (from application)
/// or SQLite datetime format (from DEFAULT datetime('now')).
fn parse_timestamp(s: &str) -> Result<DateTime<Utc>> {
    // Try RFC 3339 first (application-provided timestamps)
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }

    // Try SQLite datetime format: "YYYY-MM-DD HH:MM:SS"
    if let Ok(naive) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
        return Ok(naive.and_utc());
    }

    Err(FlowplaneError::validation(format!("Invalid timestamp format: {}", s)))
}

// ============================================================================
// Data Types
// ============================================================================

/// Proxy certificate data as stored in the database.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProxyCertificateData {
    /// Unique certificate ID
    pub id: ProxyCertificateId,

    /// Team owning this certificate
    pub team_id: TeamId,

    /// Unique proxy instance identifier
    pub proxy_id: String,

    /// Certificate serial number from Vault
    pub serial_number: String,

    /// Full SPIFFE identity URI
    pub spiffe_uri: String,

    /// When the certificate was issued
    pub issued_at: DateTime<Utc>,

    /// When the certificate expires
    pub expires_at: DateTime<Utc>,

    /// User who generated the certificate
    pub issued_by_user_id: Option<UserId>,

    /// When the certificate was revoked (None if not revoked)
    pub revoked_at: Option<DateTime<Utc>>,

    /// Reason for revocation
    pub revoked_reason: Option<String>,

    /// Record creation timestamp
    pub created_at: DateTime<Utc>,
}

impl ProxyCertificateData {
    /// Check if the certificate is currently valid (not expired and not revoked).
    pub fn is_valid(&self) -> bool {
        self.revoked_at.is_none() && self.expires_at > Utc::now()
    }

    /// Check if the certificate is expired.
    pub fn is_expired(&self) -> bool {
        self.expires_at <= Utc::now()
    }

    /// Check if the certificate is revoked.
    pub fn is_revoked(&self) -> bool {
        self.revoked_at.is_some()
    }
}

/// Request to create a new proxy certificate record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateProxyCertificateRequest {
    /// Team owning this certificate
    pub team_id: TeamId,

    /// Unique proxy instance identifier
    pub proxy_id: String,

    /// Certificate serial number from Vault
    pub serial_number: String,

    /// Full SPIFFE identity URI
    pub spiffe_uri: String,

    /// When the certificate was issued
    pub issued_at: DateTime<Utc>,

    /// When the certificate expires
    pub expires_at: DateTime<Utc>,

    /// User who generated the certificate
    pub issued_by_user_id: Option<UserId>,
}

// ============================================================================
// Database Row Type
// ============================================================================

#[derive(Debug, Clone, FromRow)]
struct ProxyCertificateRow {
    id: String,
    team_id: String,
    proxy_id: String,
    serial_number: String,
    spiffe_uri: String,
    issued_at: String,
    expires_at: String,
    issued_by_user_id: Option<String>,
    revoked_at: Option<String>,
    revoked_reason: Option<String>,
    created_at: String,
}

impl TryFrom<ProxyCertificateRow> for ProxyCertificateData {
    type Error = FlowplaneError;

    fn try_from(row: ProxyCertificateRow) -> Result<Self> {
        let issued_at = parse_timestamp(&row.issued_at)?;
        let expires_at = parse_timestamp(&row.expires_at)?;
        let created_at = parse_timestamp(&row.created_at)?;
        let revoked_at = row.revoked_at.map(|s| parse_timestamp(&s)).transpose()?;

        Ok(ProxyCertificateData {
            id: ProxyCertificateId::from_string(row.id),
            team_id: TeamId::from_string(row.team_id),
            proxy_id: row.proxy_id,
            serial_number: row.serial_number,
            spiffe_uri: row.spiffe_uri,
            issued_at,
            expires_at,
            issued_by_user_id: row.issued_by_user_id.map(UserId::from_string),
            revoked_at,
            revoked_reason: row.revoked_reason,
            created_at,
        })
    }
}

// ============================================================================
// Repository Trait
// ============================================================================

#[async_trait]
pub trait ProxyCertificateRepository: Send + Sync {
    /// Create a new proxy certificate record.
    async fn create(&self, request: CreateProxyCertificateRequest) -> Result<ProxyCertificateData>;

    /// Get a certificate by ID.
    async fn get_by_id(&self, id: &ProxyCertificateId) -> Result<Option<ProxyCertificateData>>;

    /// Get a certificate by serial number within a team.
    async fn get_by_serial(
        &self,
        team_id: &TeamId,
        serial_number: &str,
    ) -> Result<Option<ProxyCertificateData>>;

    /// List all certificates for a team.
    async fn list_by_team(
        &self,
        team_id: &TeamId,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<ProxyCertificateData>>;

    /// List certificates for a specific proxy within a team.
    async fn list_by_proxy(
        &self,
        team_id: &TeamId,
        proxy_id: &str,
    ) -> Result<Vec<ProxyCertificateData>>;

    /// List certificates expiring before the given date.
    async fn list_expiring_before(
        &self,
        expires_before: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<ProxyCertificateData>>;

    /// Count certificates for a team.
    async fn count_by_team(&self, team_id: &TeamId) -> Result<i64>;

    /// Revoke a certificate.
    async fn revoke(&self, id: &ProxyCertificateId, reason: &str) -> Result<ProxyCertificateData>;

    /// Delete a certificate record.
    async fn delete(&self, id: &ProxyCertificateId) -> Result<()>;
}

// ============================================================================
// SQLx Implementation
// ============================================================================

pub struct SqlxProxyCertificateRepository {
    pool: DbPool,
}

impl SqlxProxyCertificateRepository {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ProxyCertificateRepository for SqlxProxyCertificateRepository {
    #[instrument(skip(self, request), fields(team_id = %request.team_id, proxy_id = %request.proxy_id), name = "db_create_proxy_certificate")]
    async fn create(&self, request: CreateProxyCertificateRequest) -> Result<ProxyCertificateData> {
        let id = ProxyCertificateId::new();

        let row = sqlx::query_as::<_, ProxyCertificateRow>(
            r#"
            INSERT INTO proxy_certificates (
                id, team_id, proxy_id, serial_number, spiffe_uri,
                issued_at, expires_at, issued_by_user_id
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            RETURNING *
            "#,
        )
        .bind(id.as_str())
        .bind(request.team_id.as_str())
        .bind(&request.proxy_id)
        .bind(&request.serial_number)
        .bind(&request.spiffe_uri)
        .bind(request.issued_at.to_rfc3339())
        .bind(request.expires_at.to_rfc3339())
        .bind(request.issued_by_user_id.as_ref().map(|id| id.as_str()))
        .fetch_one(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: "Failed to create proxy certificate".to_string(),
        })?;

        row.try_into()
    }

    #[instrument(skip(self), fields(id = %id), name = "db_get_proxy_certificate_by_id")]
    async fn get_by_id(&self, id: &ProxyCertificateId) -> Result<Option<ProxyCertificateData>> {
        let row = sqlx::query_as::<_, ProxyCertificateRow>(
            "SELECT * FROM proxy_certificates WHERE id = $1",
        )
        .bind(id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: format!("Failed to fetch proxy certificate by ID: {}", id),
        })?;

        row.map(|r| r.try_into()).transpose()
    }

    #[instrument(skip(self), fields(team_id = %team_id, serial_number = %serial_number), name = "db_get_proxy_certificate_by_serial")]
    async fn get_by_serial(
        &self,
        team_id: &TeamId,
        serial_number: &str,
    ) -> Result<Option<ProxyCertificateData>> {
        let row = sqlx::query_as::<_, ProxyCertificateRow>(
            "SELECT * FROM proxy_certificates WHERE team_id = $1 AND serial_number = $2",
        )
        .bind(team_id.as_str())
        .bind(serial_number)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: format!("Failed to fetch proxy certificate by serial: {}", serial_number),
        })?;

        row.map(|r| r.try_into()).transpose()
    }

    #[instrument(skip(self), fields(team_id = %team_id, limit = limit, offset = offset), name = "db_list_proxy_certificates_by_team")]
    async fn list_by_team(
        &self,
        team_id: &TeamId,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<ProxyCertificateData>> {
        let rows = sqlx::query_as::<_, ProxyCertificateRow>(
            r#"
            SELECT * FROM proxy_certificates
            WHERE team_id = $1
            ORDER BY created_at DESC
            LIMIT $2 OFFSET $3
            "#,
        )
        .bind(team_id.as_str())
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: format!("Failed to list proxy certificates for team: {}", team_id),
        })?;

        rows.into_iter().map(|r| r.try_into()).collect()
    }

    #[instrument(skip(self), fields(team_id = %team_id, proxy_id = %proxy_id), name = "db_list_proxy_certificates_by_proxy")]
    async fn list_by_proxy(
        &self,
        team_id: &TeamId,
        proxy_id: &str,
    ) -> Result<Vec<ProxyCertificateData>> {
        let rows = sqlx::query_as::<_, ProxyCertificateRow>(
            r#"
            SELECT * FROM proxy_certificates
            WHERE team_id = $1 AND proxy_id = $2
            ORDER BY created_at DESC
            "#,
        )
        .bind(team_id.as_str())
        .bind(proxy_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: format!("Failed to list proxy certificates for proxy: {}", proxy_id),
        })?;

        rows.into_iter().map(|r| r.try_into()).collect()
    }

    #[instrument(skip(self), fields(expires_before = %expires_before, limit = limit), name = "db_list_expiring_proxy_certificates")]
    async fn list_expiring_before(
        &self,
        expires_before: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<ProxyCertificateData>> {
        let rows = sqlx::query_as::<_, ProxyCertificateRow>(
            r#"
            SELECT * FROM proxy_certificates
            WHERE expires_at < $1 AND revoked_at IS NULL
            ORDER BY expires_at ASC
            LIMIT $2
            "#,
        )
        .bind(expires_before.to_rfc3339())
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: "Failed to list expiring proxy certificates".to_string(),
        })?;

        rows.into_iter().map(|r| r.try_into()).collect()
    }

    #[instrument(skip(self), fields(team_id = %team_id), name = "db_count_proxy_certificates")]
    async fn count_by_team(&self, team_id: &TeamId) -> Result<i64> {
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM proxy_certificates WHERE team_id = $1",
        )
        .bind(team_id.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: format!("Failed to count proxy certificates for team: {}", team_id),
        })?;

        Ok(count)
    }

    #[instrument(skip(self), fields(id = %id, reason = %reason), name = "db_revoke_proxy_certificate")]
    async fn revoke(&self, id: &ProxyCertificateId, reason: &str) -> Result<ProxyCertificateData> {
        let row = sqlx::query_as::<_, ProxyCertificateRow>(
            r#"
            UPDATE proxy_certificates
            SET revoked_at = $2, revoked_reason = $3
            WHERE id = $1
            RETURNING *
            "#,
        )
        .bind(id.as_str())
        .bind(Utc::now().to_rfc3339())
        .bind(reason)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: format!("Failed to revoke proxy certificate: {}", id),
        })?;

        row.try_into()
    }

    #[instrument(skip(self), fields(id = %id), name = "db_delete_proxy_certificate")]
    async fn delete(&self, id: &ProxyCertificateId) -> Result<()> {
        let result = sqlx::query("DELETE FROM proxy_certificates WHERE id = $1")
            .bind(id.as_str())
            .execute(&self.pool)
            .await
            .map_err(|e| FlowplaneError::Database {
                source: e,
                context: format!("Failed to delete proxy certificate: {}", id),
            })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::not_found("ProxyCertificate", id.as_str()));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn setup_test_db() -> DbPool {
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect("sqlite::memory:")
            .await
            .expect("Failed to create test database");

        crate::storage::migrations::run_migrations(&pool).await.expect("Failed to run migrations");

        pool
    }

    async fn create_test_team(pool: &DbPool) -> TeamId {
        use crate::auth::team::CreateTeamRequest;
        use crate::storage::repositories::{SqlxTeamRepository, TeamRepository};

        let repo = SqlxTeamRepository::new(pool.clone());
        let request = CreateTeamRequest {
            name: format!("test-team-{}", uuid::Uuid::new_v4()),
            display_name: "Test Team".to_string(),
            description: None,
            owner_user_id: None,
            settings: None,
        };
        let team = repo.create_team(request).await.expect("create team");
        team.id
    }

    #[tokio::test]
    async fn test_create_and_get_certificate() {
        let pool = setup_test_db().await;
        let team_id = create_test_team(&pool).await;
        let repo = SqlxProxyCertificateRepository::new(pool);

        let request = CreateProxyCertificateRequest {
            team_id: team_id.clone(),
            proxy_id: "proxy-1".to_string(),
            serial_number: "12:34:56:78".to_string(),
            spiffe_uri: "spiffe://flowplane.local/team/test/proxy/proxy-1".to_string(),
            issued_at: Utc::now(),
            expires_at: Utc::now() + chrono::Duration::days(30),
            issued_by_user_id: None,
        };

        let created = repo.create(request).await.expect("create certificate");

        assert_eq!(created.proxy_id, "proxy-1");
        assert_eq!(created.serial_number, "12:34:56:78");
        assert!(created.is_valid());

        // Get by ID
        let fetched = repo.get_by_id(&created.id).await.expect("get by id");
        assert!(fetched.is_some());
        assert_eq!(fetched.unwrap().id, created.id);

        // Get by serial
        let fetched = repo.get_by_serial(&team_id, "12:34:56:78").await.expect("get by serial");
        assert!(fetched.is_some());
    }

    #[tokio::test]
    async fn test_list_certificates_by_team() {
        let pool = setup_test_db().await;
        let team_id = create_test_team(&pool).await;
        let repo = SqlxProxyCertificateRepository::new(pool);

        // Create 3 certificates
        for i in 1..=3 {
            let request = CreateProxyCertificateRequest {
                team_id: team_id.clone(),
                proxy_id: format!("proxy-{}", i),
                serial_number: format!("serial-{}", i),
                spiffe_uri: format!("spiffe://test/team/test/proxy/proxy-{}", i),
                issued_at: Utc::now(),
                expires_at: Utc::now() + chrono::Duration::days(30),
                issued_by_user_id: None,
            };
            repo.create(request).await.expect("create certificate");
        }

        let certs = repo.list_by_team(&team_id, 10, 0).await.expect("list");
        assert_eq!(certs.len(), 3);

        let count = repo.count_by_team(&team_id).await.expect("count");
        assert_eq!(count, 3);
    }

    #[tokio::test]
    async fn test_revoke_certificate() {
        let pool = setup_test_db().await;
        let team_id = create_test_team(&pool).await;
        let repo = SqlxProxyCertificateRepository::new(pool);

        let request = CreateProxyCertificateRequest {
            team_id,
            proxy_id: "proxy-1".to_string(),
            serial_number: "abc123".to_string(),
            spiffe_uri: "spiffe://test/team/test/proxy/proxy-1".to_string(),
            issued_at: Utc::now(),
            expires_at: Utc::now() + chrono::Duration::days(30),
            issued_by_user_id: None,
        };

        let created = repo.create(request).await.expect("create");
        assert!(created.is_valid());

        let revoked = repo.revoke(&created.id, "Key compromised").await.expect("revoke");
        assert!(revoked.is_revoked());
        assert!(!revoked.is_valid());
        assert_eq!(revoked.revoked_reason.as_deref(), Some("Key compromised"));
    }

    #[tokio::test]
    async fn test_is_expired() {
        let cert = ProxyCertificateData {
            id: ProxyCertificateId::new(),
            team_id: TeamId::new(),
            proxy_id: "test".to_string(),
            serial_number: "123".to_string(),
            spiffe_uri: "spiffe://test".to_string(),
            issued_at: Utc::now() - chrono::Duration::days(60),
            expires_at: Utc::now() - chrono::Duration::days(30), // Expired 30 days ago
            issued_by_user_id: None,
            revoked_at: None,
            revoked_reason: None,
            created_at: Utc::now(),
        };

        assert!(cert.is_expired());
        assert!(!cert.is_valid());
    }
}
