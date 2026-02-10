//! Team stats provider service
//!
//! This module provides the main service layer for fetching stats with:
//! - Team isolation (users can only see their teams' stats)
//! - Caching for performance
//! - Data source abstraction for testability

use crate::domain::{ClusterStats, EnvoyHealthStatus, StatsOverview, StatsSnapshot};
use crate::errors::{FlowplaneError, Result};
use crate::services::stats_cache::StatsCache;
use crate::services::stats_data_source::StatsDataSource;
use crate::storage::repositories::{InstanceAppRepository, TeamRepository};
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Configuration for the stats provider
#[derive(Debug, Clone)]
pub struct StatsProviderConfig {
    /// Base URL for Envoy admin API (if port is not in team record)
    pub envoy_admin_base_url: String,
    /// Whether to use the team's envoy_admin_port
    pub use_team_admin_port: bool,
}

impl Default for StatsProviderConfig {
    fn default() -> Self {
        Self { envoy_admin_base_url: "http://localhost".to_string(), use_team_admin_port: true }
    }
}

/// Service for fetching stats with team isolation
pub struct TeamStatsProvider<D, T, A>
where
    D: StatsDataSource,
    T: TeamRepository,
    A: InstanceAppRepository,
{
    data_source: Arc<D>,
    cache: Arc<StatsCache>,
    team_repo: Arc<T>,
    app_repo: Arc<A>,
    config: StatsProviderConfig,
}

impl<D, T, A> TeamStatsProvider<D, T, A>
where
    D: StatsDataSource,
    T: TeamRepository,
    A: InstanceAppRepository,
{
    /// Create a new TeamStatsProvider
    pub fn new(
        data_source: Arc<D>,
        cache: Arc<StatsCache>,
        team_repo: Arc<T>,
        app_repo: Arc<A>,
        config: StatsProviderConfig,
    ) -> Self {
        Self { data_source, cache, team_repo, app_repo, config }
    }

    /// Check if the stats dashboard feature is enabled
    pub async fn is_enabled(&self) -> Result<bool> {
        use crate::storage::repositories::app_ids;
        self.app_repo.is_enabled(app_ids::STATS_DASHBOARD).await
    }

    /// Get the admin URL for a team's Envoy instance
    async fn get_admin_url(&self, team_name: &str) -> Result<String> {
        if self.config.use_team_admin_port {
            let team = self
                .team_repo
                .get_team_by_name(team_name)
                .await?
                .ok_or_else(|| FlowplaneError::not_found("Team", team_name))?;

            if let Some(port) = team.envoy_admin_port {
                return Ok(format!("{}:{}", self.config.envoy_admin_base_url, port));
            }
        }

        // Fallback to base URL with default port
        Ok(format!("{}:9901", self.config.envoy_admin_base_url))
    }

    /// Get stats snapshot for a team
    ///
    /// This method enforces team isolation - callers must ensure the user
    /// has access to the requested team.
    pub async fn get_stats(&self, team_name: &str) -> Result<StatsSnapshot> {
        // Check if feature is enabled
        if !self.is_enabled().await? {
            return Err(FlowplaneError::internal("Stats dashboard feature is not enabled"));
        }

        // Check cache first
        if let Some(cached) = self.cache.get(team_name) {
            debug!("Cache hit for team {}", team_name);
            return Ok(cached);
        }

        debug!("Cache miss for team {}, fetching from source", team_name);

        // Get admin URL for team
        let admin_url = self.get_admin_url(team_name).await?;

        // Fetch from data source
        let snapshot = self.data_source.fetch_stats(team_name, &admin_url).await?;

        // Cache the result
        self.cache.set(team_name, snapshot.clone());

        Ok(snapshot)
    }

    /// Get stats overview for a team (summary data for dashboard)
    pub async fn get_overview(&self, team_name: &str) -> Result<StatsOverview> {
        let snapshot = self.get_stats(team_name).await?;
        Ok(Self::compute_overview(&snapshot))
    }

    /// Get cluster stats for a team
    pub async fn get_clusters(&self, team_name: &str) -> Result<Vec<ClusterStats>> {
        let snapshot = self.get_stats(team_name).await?;
        Ok(snapshot.clusters)
    }

    /// Get stats for a specific cluster
    pub async fn get_cluster(&self, team_name: &str, cluster_name: &str) -> Result<ClusterStats> {
        let snapshot = self.get_stats(team_name).await?;
        snapshot
            .clusters
            .into_iter()
            .find(|c| c.cluster_name == cluster_name)
            .ok_or_else(|| FlowplaneError::not_found("Cluster", cluster_name))
    }

    /// Get stats for all teams (admin only) or org-scoped teams.
    ///
    /// When `org_id` is `Some`, only teams belonging to that org are included.
    /// When `None`, all teams are returned (platform admin view).
    pub async fn get_all_teams_overview(
        &self,
        org_id: Option<&crate::domain::OrgId>,
    ) -> Result<Vec<(String, StatsOverview)>> {
        // Check if feature is enabled
        if !self.is_enabled().await? {
            return Err(FlowplaneError::internal("Stats dashboard feature is not enabled"));
        }

        // List teams scoped by org when provided, otherwise all
        let teams = match org_id {
            Some(id) => self.team_repo.list_teams_by_org(id).await?,
            None => self.team_repo.list_teams(1000, 0).await?,
        };
        let mut overviews = Vec::new();

        for team in teams {
            match self.get_stats(&team.name).await {
                Ok(snapshot) => {
                    overviews.push((team.name, Self::compute_overview(&snapshot)));
                }
                Err(e) => {
                    warn!("Failed to fetch stats for team {}: {}", team.name, e);
                    // Continue with other teams even if one fails
                }
            }
        }

        Ok(overviews)
    }

    /// Check health of Envoy for a team
    pub async fn health_check(&self, team_name: &str) -> Result<bool> {
        let admin_url = self.get_admin_url(team_name).await?;
        self.data_source.health_check(&admin_url).await
    }

    /// Invalidate cache for a team
    pub fn invalidate_cache(&self, team_name: &str) {
        self.cache.invalidate(team_name);
        info!("Invalidated stats cache for team {}", team_name);
    }

    /// Perform cache maintenance
    pub fn cleanup_cache(&self) {
        self.cache.cleanup();
    }

    /// Compute overview statistics from a snapshot
    fn compute_overview(snapshot: &StatsSnapshot) -> StatsOverview {
        let total_requests = snapshot.response_codes.xx_2xx
            + snapshot.response_codes.xx_3xx
            + snapshot.response_codes.xx_4xx
            + snapshot.response_codes.xx_5xx;

        let error_rate = if total_requests > 0 {
            (snapshot.response_codes.xx_4xx + snapshot.response_codes.xx_5xx) as f64
                / total_requests as f64
        } else {
            0.0
        };

        let mut healthy_clusters = 0u64;
        let mut degraded_clusters = 0u64;
        let mut unhealthy_clusters = 0u64;

        for cluster in &snapshot.clusters {
            let status =
                EnvoyHealthStatus::from_host_counts(cluster.healthy_hosts, cluster.total_hosts);
            match status {
                EnvoyHealthStatus::Healthy => healthy_clusters += 1,
                EnvoyHealthStatus::Degraded => degraded_clusters += 1,
                EnvoyHealthStatus::Unhealthy => unhealthy_clusters += 1,
                EnvoyHealthStatus::Unknown => {}
            }
        }

        StatsOverview {
            total_rps: snapshot.requests.rps.unwrap_or(0.0),
            total_connections: snapshot.connections.downstream_cx_active,
            error_rate,
            p99_latency_ms: snapshot.latency.p99_ms.unwrap_or(0.0),
            healthy_clusters,
            degraded_clusters,
            unhealthy_clusters,
            total_clusters: snapshot.clusters.len() as u64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::team::{CreateTeamRequest, Team, TeamStatus, UpdateTeamRequest};
    use crate::domain::{ClusterStats, OrgId, ResponseCodeMetrics, StatsSnapshot, TeamId};
    use crate::services::stats_cache::StatsCacheConfig;
    use crate::services::stats_data_source::MockStatsDataSource;
    use crate::storage::repositories::TeamRepository;
    use async_trait::async_trait;
    use std::sync::Mutex;
    use std::time::Duration;

    // Mock team repository for testing
    struct MockTeamRepo {
        teams: Mutex<Vec<Team>>,
    }

    impl MockTeamRepo {
        fn new() -> Self {
            Self { teams: Mutex::new(vec![]) }
        }

        fn add_team(&self, name: &str, admin_port: Option<u16>) {
            self.add_team_with_org(name, admin_port, "test-org");
        }

        fn add_team_with_org(&self, name: &str, admin_port: Option<u16>, org: &str) {
            let team = Team {
                id: TeamId::new(),
                name: name.to_string(),
                display_name: name.to_string(),
                description: None,
                owner_user_id: None,
                org_id: OrgId::from_str_unchecked(org),
                settings: None,
                status: TeamStatus::Active,
                envoy_admin_port: admin_port,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            };
            self.teams.lock().unwrap().push(team);
        }
    }

    #[async_trait]
    impl TeamRepository for MockTeamRepo {
        async fn create_team(&self, _request: CreateTeamRequest) -> Result<Team> {
            unimplemented!()
        }

        async fn get_team_by_id(&self, _id: &TeamId) -> Result<Option<Team>> {
            unimplemented!()
        }

        async fn get_team_by_name(&self, name: &str) -> Result<Option<Team>> {
            Ok(self.teams.lock().unwrap().iter().find(|t| t.name == name).cloned())
        }

        async fn list_teams(&self, _limit: i64, _offset: i64) -> Result<Vec<Team>> {
            Ok(self.teams.lock().unwrap().clone())
        }

        async fn list_teams_by_status(
            &self,
            _status: TeamStatus,
            _limit: i64,
            _offset: i64,
        ) -> Result<Vec<Team>> {
            unimplemented!()
        }

        async fn count_teams(&self) -> Result<i64> {
            unimplemented!()
        }

        async fn update_team(&self, _id: &TeamId, _update: UpdateTeamRequest) -> Result<Team> {
            unimplemented!()
        }

        async fn delete_team(&self, _id: &TeamId) -> Result<()> {
            unimplemented!()
        }

        async fn is_name_available(&self, _name: &str) -> Result<bool> {
            unimplemented!()
        }

        async fn list_teams_by_org(&self, org_id: &crate::domain::OrgId) -> Result<Vec<Team>> {
            Ok(self.teams.lock().unwrap().iter().filter(|t| t.org_id == *org_id).cloned().collect())
        }

        async fn resolve_team_ids(
            &self,
            _org_id: Option<&crate::domain::OrgId>,
            _team_names: &[String],
        ) -> Result<Vec<String>> {
            unimplemented!()
        }

        async fn resolve_team_names(
            &self,
            _org_id: Option<&crate::domain::OrgId>,
            _team_ids: &[String],
        ) -> Result<Vec<String>> {
            unimplemented!()
        }

        async fn is_name_available_in_org(&self, _org_id: &OrgId, _name: &str) -> Result<bool> {
            unimplemented!()
        }

        async fn get_team_by_org_and_name(
            &self,
            _org_id: &OrgId,
            _name: &str,
        ) -> Result<Option<Team>> {
            unimplemented!()
        }
    }

    // Mock app repository for testing
    struct MockAppRepo {
        enabled: Mutex<bool>,
    }

    impl MockAppRepo {
        fn new(enabled: bool) -> Self {
            Self { enabled: Mutex::new(enabled) }
        }
    }

    #[async_trait]
    impl InstanceAppRepository for MockAppRepo {
        async fn is_enabled(&self, _app_id: &str) -> Result<bool> {
            Ok(*self.enabled.lock().unwrap())
        }

        async fn get_app(
            &self,
            _app_id: &str,
        ) -> Result<Option<crate::storage::repositories::InstanceApp>> {
            unimplemented!()
        }

        async fn get_all_apps(&self) -> Result<Vec<crate::storage::repositories::InstanceApp>> {
            unimplemented!()
        }

        async fn get_enabled_apps(&self) -> Result<Vec<crate::storage::repositories::InstanceApp>> {
            unimplemented!()
        }

        async fn enable_app(
            &self,
            _app_id: &str,
            _user_id: &str,
            _config: Option<serde_json::Value>,
        ) -> Result<crate::storage::repositories::InstanceApp> {
            unimplemented!()
        }

        async fn disable_app(
            &self,
            _app_id: &str,
            _user_id: &str,
        ) -> Result<crate::storage::repositories::InstanceApp> {
            unimplemented!()
        }

        async fn update_config(
            &self,
            _app_id: &str,
            _config: serde_json::Value,
        ) -> Result<crate::storage::repositories::InstanceApp> {
            unimplemented!()
        }

        async fn get_stats_dashboard_config(
            &self,
        ) -> Result<Option<crate::storage::repositories::StatsDashboardConfig>> {
            unimplemented!()
        }

        async fn get_external_secrets_config(
            &self,
        ) -> Result<Option<crate::storage::repositories::ExternalSecretsConfig>> {
            unimplemented!()
        }
    }

    fn create_test_snapshot(team_id: &str) -> StatsSnapshot {
        let mut snapshot = StatsSnapshot::new(team_id.to_string());
        snapshot.clusters = vec![
            ClusterStats {
                cluster_name: "api-backend".to_string(),
                healthy_hosts: 3,
                total_hosts: 3,
                ..Default::default()
            },
            ClusterStats {
                cluster_name: "db-backend".to_string(),
                healthy_hosts: 2,
                total_hosts: 3,
                ..Default::default()
            },
        ];
        snapshot.response_codes =
            ResponseCodeMetrics { xx_2xx: 900, xx_3xx: 50, xx_4xx: 40, xx_5xx: 10 };
        snapshot
    }

    #[tokio::test]
    async fn test_get_stats_when_disabled() {
        let data_source = Arc::new(MockStatsDataSource::new());
        let cache = Arc::new(StatsCache::with_defaults());
        let team_repo = Arc::new(MockTeamRepo::new());
        let app_repo = Arc::new(MockAppRepo::new(false)); // Disabled

        let provider = TeamStatsProvider::new(
            data_source,
            cache,
            team_repo,
            app_repo,
            StatsProviderConfig::default(),
        );

        let result = provider.get_stats("test-team").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_get_stats_uses_cache() {
        let data_source = Arc::new(MockStatsDataSource::new());
        let cache = Arc::new(StatsCache::new(StatsCacheConfig {
            ttl: Duration::from_secs(60),
            max_entries: 100,
        }));
        let team_repo = Arc::new(MockTeamRepo::new());
        team_repo.add_team("test-team", Some(9901));
        let app_repo = Arc::new(MockAppRepo::new(true));

        // Pre-populate cache
        let snapshot = create_test_snapshot("test-team");
        cache.set("test-team", snapshot.clone());

        let provider = TeamStatsProvider::new(
            data_source,
            cache,
            team_repo,
            app_repo,
            StatsProviderConfig::default(),
        );

        let result = provider.get_stats("test-team").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().clusters.len(), 2);
    }

    #[tokio::test]
    async fn test_get_overview() {
        let data_source = Arc::new(MockStatsDataSource::new());
        let cache = Arc::new(StatsCache::with_defaults());
        let team_repo = Arc::new(MockTeamRepo::new());
        team_repo.add_team("test-team", Some(9901));
        let app_repo = Arc::new(MockAppRepo::new(true));

        // Pre-populate cache with known data
        let snapshot = create_test_snapshot("test-team");
        cache.set("test-team", snapshot);

        let provider = TeamStatsProvider::new(
            data_source,
            cache,
            team_repo,
            app_repo,
            StatsProviderConfig::default(),
        );

        let overview = provider.get_overview("test-team").await.unwrap();

        assert_eq!(overview.total_clusters, 2);
        assert_eq!(overview.healthy_clusters, 1);
        assert_eq!(overview.degraded_clusters, 1);
        // Error rate = (40 + 10) / 1000 = 0.05
        assert!((overview.error_rate - 0.05).abs() < 0.001);
    }

    #[tokio::test]
    async fn test_get_cluster() {
        let data_source = Arc::new(MockStatsDataSource::new());
        let cache = Arc::new(StatsCache::with_defaults());
        let team_repo = Arc::new(MockTeamRepo::new());
        team_repo.add_team("test-team", Some(9901));
        let app_repo = Arc::new(MockAppRepo::new(true));

        cache.set("test-team", create_test_snapshot("test-team"));

        let provider = TeamStatsProvider::new(
            data_source,
            cache,
            team_repo,
            app_repo,
            StatsProviderConfig::default(),
        );

        let cluster = provider.get_cluster("test-team", "api-backend").await;
        assert!(cluster.is_ok());
        assert_eq!(cluster.unwrap().cluster_name, "api-backend");

        let missing = provider.get_cluster("test-team", "nonexistent").await;
        assert!(missing.is_err());
    }

    #[tokio::test]
    async fn test_invalidate_cache() {
        let data_source = Arc::new(MockStatsDataSource::new());
        let cache = Arc::new(StatsCache::with_defaults());
        let team_repo = Arc::new(MockTeamRepo::new());
        let app_repo = Arc::new(MockAppRepo::new(true));

        cache.set("test-team", create_test_snapshot("test-team"));
        assert!(cache.get("test-team").is_some());

        let provider = TeamStatsProvider::new(
            data_source,
            cache.clone(),
            team_repo,
            app_repo,
            StatsProviderConfig::default(),
        );

        provider.invalidate_cache("test-team");
        assert!(cache.get("test-team").is_none());
    }

    #[tokio::test]
    async fn test_get_all_teams_overview_org_scoped() {
        let data_source = Arc::new(MockStatsDataSource::new());
        let cache = Arc::new(StatsCache::with_defaults());
        let team_repo = Arc::new(MockTeamRepo::new());
        // Two teams in org-a, one team in org-b
        team_repo.add_team_with_org("team-alpha", Some(9901), "org-a");
        team_repo.add_team_with_org("team-beta", Some(9902), "org-a");
        team_repo.add_team_with_org("team-gamma", Some(9903), "org-b");
        let app_repo = Arc::new(MockAppRepo::new(true));

        // Pre-populate cache for all teams
        cache.set("team-alpha", create_test_snapshot("team-alpha"));
        cache.set("team-beta", create_test_snapshot("team-beta"));
        cache.set("team-gamma", create_test_snapshot("team-gamma"));

        let provider = TeamStatsProvider::new(
            data_source,
            cache,
            team_repo,
            app_repo,
            StatsProviderConfig::default(),
        );

        // Org-scoped: only org-a teams
        let org_a = OrgId::from_str_unchecked("org-a");
        let overviews = provider.get_all_teams_overview(Some(&org_a)).await.unwrap();
        assert_eq!(overviews.len(), 2);
        let names: Vec<&str> = overviews.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"team-alpha"));
        assert!(names.contains(&"team-beta"));
        assert!(!names.contains(&"team-gamma"));

        // Platform admin (no org): all teams
        let all_overviews = provider.get_all_teams_overview(None).await.unwrap();
        assert_eq!(all_overviews.len(), 3);
    }
}
