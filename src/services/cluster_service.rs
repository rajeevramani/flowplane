//! Cluster business logic service
//!
//! This module contains the business logic for cluster operations,
//! separated from HTTP concerns.

use serde_json::Value;
use std::sync::Arc;
use tracing::{error, info};

use crate::{
    errors::Error,
    observability::http_tracing::create_operation_span,
    openapi::defaults::is_default_gateway_cluster,
    storage::{ClusterData, ClusterRepository, CreateClusterRequest, UpdateClusterRequest},
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

    /// Create a new cluster
    pub async fn create_cluster(
        &self,
        name: String,
        service_name: String,
        config: ClusterSpec,
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
                team: None, // Native API clusters don't have team assignment by default
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

    /// Refresh xDS caches after cluster changes
    async fn refresh_xds(&self) -> Result<(), Error> {
        self.xds_state.refresh_clusters_from_repository().await.map_err(|err| {
            error!(error = %err, "Failed to refresh xDS caches after cluster operation");
            err
        })
    }
}
