//! Cluster business logic service
//!
//! This module contains the business logic for cluster operations,
//! separated from HTTP concerns.

use serde_json::Value;
use std::sync::Arc;
use tracing::{error, info};

use crate::{
    domain::ClusterDependency,
    errors::Error,
    observability::http_tracing::create_operation_span,
    openapi::defaults::is_default_gateway_cluster,
    services::ClusterEndpointSyncService,
    storage::{
        ClusterData, ClusterRepository, CreateClusterRequest, FilterRepository,
        RouteConfigRepository, UpdateClusterRequest,
    },
    xds::{ClusterSpec, XdsState},
};
use opentelemetry::{
    trace::{Span, SpanKind},
    KeyValue,
};

/// Service for managing cluster business logic
pub struct ClusterService {
    xds_state: Arc<XdsState>,
}

impl ClusterService {
    /// Create a new cluster service
    pub fn new(xds_state: Arc<XdsState>) -> Self {
        Self { xds_state }
    }

    /// Get the cluster repository
    fn repository(&self) -> Result<ClusterRepository, Error> {
        self.xds_state
            .cluster_repository
            .as_ref()
            .cloned()
            .ok_or_else(|| Error::internal("Cluster repository not configured"))
    }

    /// Get the route config repository
    fn route_config_repository(&self) -> Result<RouteConfigRepository, Error> {
        self.xds_state
            .route_config_repository
            .as_ref()
            .cloned()
            .ok_or_else(|| Error::internal("Route config repository not configured"))
    }

    /// Get the filter repository
    fn filter_repository(&self) -> Result<FilterRepository, Error> {
        self.xds_state
            .filter_repository
            .as_ref()
            .cloned()
            .ok_or_else(|| Error::internal("Filter repository not configured"))
    }

    /// Find all dependencies on a cluster.
    ///
    /// Returns a list of resources that reference this cluster.
    /// Returns empty list if repositories are not configured (e.g., in tests).
    pub async fn find_cluster_dependencies(&self, cluster_name: &str) -> Vec<ClusterDependency> {
        let mut dependencies = Vec::new();

        // Check route configs (skip if repository not configured)
        if let Ok(route_config_repo) = self.route_config_repository() {
            if let Ok(route_configs) = route_config_repo.find_by_cluster(cluster_name).await {
                for rc in route_configs {
                    dependencies.push(ClusterDependency::route_config(rc.id.as_str(), &rc.name));
                }
            }
        }

        // Check filters (OAuth2, JWT, ExtAuthz) - skip if repository not configured
        if let Ok(filter_repo) = self.filter_repository() {
            if let Ok(filters) = filter_repo.find_by_cluster(cluster_name).await {
                for (filter, reference_path) in filters {
                    dependencies.push(ClusterDependency::filter(
                        filter.id.as_str(),
                        &filter.name,
                        reference_path,
                    ));
                }
            }
        }

        dependencies
    }

    /// Create a new cluster
    pub async fn create_cluster(
        &self,
        name: String,
        service_name: String,
        config: ClusterSpec,
        team: Option<String>,
    ) -> Result<ClusterData, Error> {
        use opentelemetry::trace::{FutureExt, TraceContextExt};

        // Create span for the entire business logic operation
        let mut span = create_operation_span("cluster_service.create_cluster", SpanKind::Internal);
        span.set_attribute(KeyValue::new("cluster.name", name.clone()));
        span.set_attribute(KeyValue::new("cluster.service_name", service_name.clone()));

        // Create context with this span as the active span
        let cx = opentelemetry::Context::current().with_span(span);

        // Execute the rest within the span's context so child spans are properly nested
        async move {
            let repository = self.repository()?;

            let configuration: Value = config.to_value()?;

            let request = CreateClusterRequest {
                name: name.clone(),
                service_name: service_name.clone(),
                configuration,
                team,
                import_id: None,
            };

            // Create nested span for database operation
            let mut db_span = create_operation_span("db.cluster.insert", SpanKind::Client);
            db_span.set_attribute(KeyValue::new("db.operation", "INSERT"));
            db_span.set_attribute(KeyValue::new("db.table", "clusters"));
            let created = repository.create(request).await?;
            drop(db_span); // End database span

            info!(
                cluster_id = %created.id,
                cluster_name = %created.name,
                "Cluster created"
            );

            // Sync cluster endpoints
            self.sync_cluster_endpoints(&created).await;

            // Create nested span for xDS refresh
            let mut xds_span = create_operation_span("xds.refresh_clusters", SpanKind::Internal);
            xds_span.set_attribute(KeyValue::new("cluster.id", created.id.to_string()));
            self.refresh_xds().await?;
            drop(xds_span); // End xDS span

            Ok(created)
        }
        .with_context(cx)
        .await
    }

    /// List all clusters
    pub async fn list_clusters(
        &self,
        limit: Option<i32>,
        offset: Option<i32>,
    ) -> Result<Vec<ClusterData>, Error> {
        let repository = self.repository()?;
        repository.list(limit, offset).await
    }

    /// Get a cluster by name
    pub async fn get_cluster(&self, name: &str) -> Result<ClusterData, Error> {
        let repository = self.repository()?;
        repository.get_by_name(name).await
    }

    /// Update an existing cluster
    pub async fn update_cluster(
        &self,
        name: &str,
        service_name: String,
        config: ClusterSpec,
    ) -> Result<ClusterData, Error> {
        use opentelemetry::trace::{FutureExt, TraceContextExt};

        let mut span = create_operation_span("cluster_service.update_cluster", SpanKind::Internal);
        span.set_attribute(KeyValue::new("cluster.name", name.to_string()));
        span.set_attribute(KeyValue::new("cluster.service_name", service_name.clone()));

        let cx = opentelemetry::Context::current().with_span(span);

        async move {
            let repository = self.repository()?;
            let existing = repository.get_by_name(name).await?;

            let configuration = config.to_value()?;
            let update_request = UpdateClusterRequest {
                service_name: Some(service_name),
                configuration: Some(configuration),
                team: None, // Don't modify team on update unless explicitly set
            };

            let mut db_span = create_operation_span("db.cluster.update", SpanKind::Client);
            db_span.set_attribute(KeyValue::new("db.operation", "UPDATE"));
            db_span.set_attribute(KeyValue::new("db.table", "clusters"));
            let updated = repository.update(&existing.id, update_request).await?;
            drop(db_span);

            info!(
                cluster_id = %updated.id,
                cluster_name = %updated.name,
                "Cluster updated"
            );

            // Re-sync cluster endpoints
            self.sync_cluster_endpoints(&updated).await;

            let mut xds_span = create_operation_span("xds.refresh_clusters", SpanKind::Internal);
            xds_span.set_attribute(KeyValue::new("cluster.id", updated.id.to_string()));
            self.refresh_xds().await?;
            drop(xds_span);

            Ok(updated)
        }
        .with_context(cx)
        .await
    }

    /// Delete a cluster by name
    ///
    /// Fails if the cluster is referenced by any route configs or filters.
    pub async fn delete_cluster(&self, name: &str) -> Result<(), Error> {
        use opentelemetry::trace::{FutureExt, TraceContextExt};

        if is_default_gateway_cluster(name) {
            return Err(Error::validation("The default gateway cluster cannot be deleted"));
        }

        let mut span = create_operation_span("cluster_service.delete_cluster", SpanKind::Internal);
        span.set_attribute(KeyValue::new("cluster.name", name.to_string()));

        let cx = opentelemetry::Context::current().with_span(span);

        async move {
            let repository = self.repository()?;
            let existing = repository.get_by_name(name).await?;

            // Check for dependencies before deleting
            let dependencies = self.find_cluster_dependencies(name).await;
            if !dependencies.is_empty() {
                let mut parts = vec![];
                let route_config_count =
                    dependencies.iter().filter(|d| d.resource_type == "route_config").count();
                let filter_count =
                    dependencies.iter().filter(|d| d.resource_type == "filter").count();

                if route_config_count > 0 {
                    parts.push(format!("{} route config(s)", route_config_count));
                }
                if filter_count > 0 {
                    parts.push(format!("{} filter(s)", filter_count));
                }

                return Err(Error::conflict(
                    format!(
                        "Cluster '{}' is in use by {}. Remove references before deleting.",
                        name,
                        parts.join(" and ")
                    ),
                    "cluster",
                ));
            }

            // Clear cluster endpoints before deleting
            self.clear_cluster_endpoints(&existing.id).await;

            let mut db_span = create_operation_span("db.cluster.delete", SpanKind::Client);
            db_span.set_attribute(KeyValue::new("db.operation", "DELETE"));
            db_span.set_attribute(KeyValue::new("db.table", "clusters"));
            db_span.set_attribute(KeyValue::new("cluster.id", existing.id.to_string()));
            repository.delete(&existing.id).await?;
            drop(db_span);

            info!(
                cluster_id = %existing.id,
                cluster_name = %existing.name,
                "Cluster deleted"
            );

            let mut xds_span = create_operation_span("xds.refresh_clusters", SpanKind::Internal);
            xds_span.set_attribute(KeyValue::new("cluster.id", existing.id.to_string()));
            self.refresh_xds().await?;
            drop(xds_span);

            Ok(())
        }
        .with_context(cx)
        .await
    }

    /// Parse cluster configuration from stored JSON
    pub fn parse_config(&self, data: &ClusterData) -> Result<ClusterSpec, Error> {
        let value: Value = serde_json::from_str(&data.configuration).map_err(|err| {
            Error::internal(format!("Failed to parse stored cluster configuration: {}", err))
        })?;
        ClusterSpec::from_value(value)
    }

    /// Sync cluster endpoint records from the cluster's stored configuration.
    /// This is best-effort: failures are logged but don't block the cluster operation.
    async fn sync_cluster_endpoints(&self, cluster: &ClusterData) {
        let pool = match self.xds_state.cluster_repository.as_ref().map(|r| r.pool().clone()) {
            Some(pool) => pool,
            None => return,
        };

        let sync_service = ClusterEndpointSyncService::new(pool);
        match sync_service.sync(&cluster.id, &cluster.configuration).await {
            Ok(result) => {
                info!(
                    cluster_id = %cluster.id,
                    cluster_name = %cluster.name,
                    endpoints_created = result.endpoints_created,
                    endpoints_updated = result.endpoints_updated,
                    endpoints_deleted = result.endpoints_deleted,
                    "Cluster endpoints synced"
                );
            }
            Err(e) => {
                error!(
                    cluster_id = %cluster.id,
                    cluster_name = %cluster.name,
                    error = %e,
                    "Failed to sync cluster endpoints"
                );
            }
        }
    }

    /// Clear cluster endpoint records before deletion.
    /// Best-effort: failures are logged but don't block the delete operation.
    async fn clear_cluster_endpoints(&self, cluster_id: &crate::domain::ClusterId) {
        let pool = match self.xds_state.cluster_repository.as_ref().map(|r| r.pool().clone()) {
            Some(pool) => pool,
            None => return,
        };

        let sync_service = ClusterEndpointSyncService::new(pool);
        match sync_service.clear(cluster_id).await {
            Ok(deleted) => {
                info!(cluster_id = %cluster_id, deleted = deleted, "Cluster endpoints cleared");
            }
            Err(e) => {
                error!(cluster_id = %cluster_id, error = %e, "Failed to clear cluster endpoints");
            }
        }
    }

    /// Refresh xDS caches after cluster changes
    async fn refresh_xds(&self) -> Result<(), Error> {
        self.xds_state.refresh_clusters_from_repository().await.map_err(|err| {
            error!(error = %err, "Failed to refresh xDS caches after cluster operation");
            err
        })
    }
}
