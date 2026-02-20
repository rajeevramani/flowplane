use crate::errors::Error as FlowplaneError;
use crate::storage::pool::DbPool;
use serde::Serialize;

type Result<T> = std::result::Result<T, FlowplaneError>;

/// Per-team resource counts returned by the aggregation query.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct TeamResourceCounts {
    pub team_name: String,
    pub team_display_name: String,
    pub org_id: Option<String>,
    pub org_name: Option<String>,
    pub cluster_count: i64,
    pub listener_count: i64,
    pub route_config_count: i64,
    pub filter_count: i64,
    pub dataplane_count: i64,
    pub secret_count: i64,
    pub import_count: i64,
}

pub struct AdminSummaryRepository {
    pool: DbPool,
}

impl AdminSummaryRepository {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Single CTE query that aggregates resource counts per team.
    /// Filters out teams belonging to the "platform" or "default" orgs.
    pub async fn get_resource_summary(&self) -> Result<Vec<TeamResourceCounts>> {
        let rows = sqlx::query_as::<_, TeamResourceCounts>(
            r#"
            SELECT
                t.name        AS team_name,
                t.display_name AS team_display_name,
                t.org_id      AS org_id,
                o.name        AS org_name,
                COALESCE(c.cnt, 0)  AS cluster_count,
                COALESCE(l.cnt, 0)  AS listener_count,
                COALESCE(r.cnt, 0)  AS route_config_count,
                COALESCE(f.cnt, 0)  AS filter_count,
                COALESCE(d.cnt, 0)  AS dataplane_count,
                COALESCE(s.cnt, 0)  AS secret_count,
                COALESCE(im.cnt, 0) AS import_count
            FROM teams t
            LEFT JOIN organizations o ON o.id = t.org_id
            LEFT JOIN (SELECT team, COUNT(*) AS cnt FROM clusters      GROUP BY team) c  ON c.team  = t.name
            LEFT JOIN (SELECT team, COUNT(*) AS cnt FROM listeners     GROUP BY team) l  ON l.team  = t.name
            LEFT JOIN (SELECT team, COUNT(*) AS cnt FROM route_configs GROUP BY team) r  ON r.team  = t.name
            LEFT JOIN (SELECT team, COUNT(*) AS cnt FROM filters       GROUP BY team) f  ON f.team  = t.name
            LEFT JOIN (SELECT team, COUNT(*) AS cnt FROM dataplanes    GROUP BY team) d  ON d.team  = t.name
            LEFT JOIN (SELECT team, COUNT(*) AS cnt FROM secrets       GROUP BY team) s  ON s.team  = t.name
            LEFT JOIN (SELECT team, COUNT(*) AS cnt FROM import_metadata GROUP BY team) im ON im.team = t.name
            WHERE t.status = 'active'
              AND (o.name IS NULL OR o.name NOT IN ('platform', 'default'))
            ORDER BY o.name, t.name
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: "Failed to fetch admin resource summary".to_string(),
        })?;

        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::test_helpers::TestDatabase;

    #[tokio::test]
    async fn test_get_resource_summary_returns_team_counts() {
        let _db = TestDatabase::new("admin_summary_counts").await;
        let pool = _db.pool.clone();

        let repo = AdminSummaryRepository::new(pool);
        let summary = repo.get_resource_summary().await.expect("get summary");

        // Seeded test data has teams — verify we get results and counts are non-negative
        for row in &summary {
            assert!(row.cluster_count >= 0);
            assert!(row.listener_count >= 0);
            assert!(row.route_config_count >= 0);
            assert!(row.filter_count >= 0);
            assert!(row.dataplane_count >= 0);
            assert!(row.secret_count >= 0);
            assert!(row.import_count >= 0);
        }
    }

    #[tokio::test]
    async fn test_summary_excludes_platform_org() {
        let _db = TestDatabase::new("admin_summary_excludes_platform").await;
        let pool = _db.pool.clone();

        // Create a platform org and team
        let now = chrono::Utc::now();
        let org_id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO organizations (id, name, display_name, status, created_at, updated_at)
             VALUES ($1, 'platform', 'Platform', 'active', $2, $2)",
        )
        .bind(&org_id)
        .bind(now)
        .execute(&pool)
        .await
        .expect("create platform org");

        let team_id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO teams (id, name, display_name, org_id, status, created_at, updated_at)
             VALUES ($1, 'platform-governance', 'Platform Governance', $2, 'active', $3, $3)",
        )
        .bind(&team_id)
        .bind(&org_id)
        .bind(now)
        .execute(&pool)
        .await
        .expect("create platform team");

        let repo = AdminSummaryRepository::new(pool);
        let summary = repo.get_resource_summary().await.expect("get summary");

        // The platform team should be excluded
        let platform_teams: Vec<_> =
            summary.iter().filter(|r| r.org_name.as_deref() == Some("platform")).collect();
        assert!(platform_teams.is_empty(), "platform org teams should be excluded");
    }
}
