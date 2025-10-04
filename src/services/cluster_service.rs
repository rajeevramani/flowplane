//! Cluster business logic service
//!
//! This module contains the business logic for cluster operations,
//! separated from HTTP concerns.

use serde_json::Value;
use std::sync::Arc;
use tracing::{error, info};

use crate::{
    errors::Error,
    openapi::defaults::is_default_gateway_cluster,
    storage::{ClusterData, ClusterRepository, CreateClusterRequest, UpdateClusterRequest},
    xds::{ClusterSpec, XdsState},
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
        let repository = self.repository()?;

        let configuration: Value = config.to_value()?;

        let request = CreateClusterRequest {
            name: name.clone(),
            service_name: service_name.clone(),
            configuration,
        };

        let created = repository.create(request).await?;

        info!(
            cluster_id = %created.id,
            cluster_name = %created.name,
            "Cluster created"
        );

        self.refresh_xds().await?;

        Ok(created)
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
        let repository = self.repository()?;
        let existing = repository.get_by_name(name).await?;

        let configuration = config.to_value()?;
        let update_request = UpdateClusterRequest {
            service_name: Some(service_name),
            configuration: Some(configuration),
        };

        let updated = repository.update(&existing.id, update_request).await?;

        info!(
            cluster_id = %updated.id,
            cluster_name = %updated.name,
            "Cluster updated"
        );

        self.refresh_xds().await?;

        Ok(updated)
    }

    /// Delete a cluster by name
    pub async fn delete_cluster(&self, name: &str) -> Result<(), Error> {
        if is_default_gateway_cluster(name) {
            return Err(Error::validation("The default gateway cluster cannot be deleted"));
        }

        let repository = self.repository()?;
        let existing = repository.get_by_name(name).await?;

        repository.delete(&existing.id).await?;

        info!(
            cluster_id = %existing.id,
            cluster_name = %existing.name,
            "Cluster deleted"
        );

        self.refresh_xds().await?;

        Ok(())
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
