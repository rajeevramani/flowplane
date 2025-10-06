//! Route business logic service
//!
//! This module contains the business logic for route operations,
//! separated from HTTP concerns.

use serde_json::Value;
use std::sync::Arc;
use tracing::{error, info};

use crate::{
    errors::Error,
    openapi::defaults::is_default_gateway_route,
    storage::{
        CreateRouteRepositoryRequest, RouteData, RouteRepository, UpdateRouteRepositoryRequest,
    },
    xds::{route::RouteConfig, XdsState},
};

/// Service for managing route business logic
pub struct RouteService {
    xds_state: Arc<XdsState>,
}

impl RouteService {
    /// Create a new route service
    pub fn new(xds_state: Arc<XdsState>) -> Self {
        Self { xds_state }
    }

    /// Get the route repository
    fn repository(&self) -> Result<RouteRepository, Error> {
        self.xds_state
            .route_repository
            .as_ref()
            .cloned()
            .ok_or_else(|| Error::internal("Route repository not configured"))
    }

    /// Create a new route
    pub async fn create_route(
        &self,
        name: String,
        path_prefix: String,
        cluster_summary: String,
        config: Value,
    ) -> Result<RouteData, Error> {
        let repository = self.repository()?;

        let request = CreateRouteRepositoryRequest {
            name: name.clone(),
            path_prefix,
            cluster_name: cluster_summary,
            configuration: config,
        };

        let created = repository.create(request).await?;

        info!(
            route_id = %created.id,
            route_name = %created.name,
            "Route created"
        );

        self.refresh_xds().await?;

        Ok(created)
    }

    /// List all routes
    pub async fn list_routes(
        &self,
        limit: Option<i32>,
        offset: Option<i32>,
    ) -> Result<Vec<RouteData>, Error> {
        let repository = self.repository()?;
        repository.list(limit, offset).await
    }

    /// Get a route by name
    pub async fn get_route(&self, name: &str) -> Result<RouteData, Error> {
        let repository = self.repository()?;
        repository.get_by_name(name).await
    }

    /// Update an existing route
    pub async fn update_route(
        &self,
        name: &str,
        path_prefix: String,
        cluster_summary: String,
        config: Value,
    ) -> Result<RouteData, Error> {
        let repository = self.repository()?;
        let existing = repository.get_by_name(name).await?;

        let update_request = UpdateRouteRepositoryRequest {
            path_prefix: Some(path_prefix),
            cluster_name: Some(cluster_summary),
            configuration: Some(config),
        };

        let updated = repository.update(&existing.id, update_request).await?;

        info!(
            route_id = %updated.id,
            route_name = %updated.name,
            "Route updated"
        );

        self.refresh_xds().await?;

        Ok(updated)
    }

    /// Delete a route by name
    pub async fn delete_route(&self, name: &str) -> Result<(), Error> {
        if is_default_gateway_route(name) {
            return Err(Error::validation("The default gateway route cannot be deleted"));
        }

        let repository = self.repository()?;
        let existing = repository.get_by_name(name).await?;

        repository.delete(&existing.id).await?;

        info!(
            route_id = %existing.id,
            route_name = %existing.name,
            "Route deleted"
        );

        self.refresh_xds().await?;

        Ok(())
    }

    /// Parse route configuration from stored JSON
    pub fn parse_config(&self, data: &RouteData) -> Result<RouteConfig, Error> {
        serde_json::from_str(&data.configuration).map_err(|err| {
            Error::internal(format!("Failed to parse stored route configuration: {}", err))
        })
    }

    /// Refresh xDS caches after route changes
    async fn refresh_xds(&self) -> Result<(), Error> {
        self.xds_state.refresh_routes_from_repository().await.map_err(|err| {
            error!(error = %err, "Failed to refresh xDS caches after route operation");
            err
        })
    }
}
