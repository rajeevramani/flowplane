//! Route business logic service
//!
//! This module contains the business logic for route operations,
//! separated from HTTP concerns.

use serde_json::Value;
use std::sync::Arc;
use tracing::{error, info};

use crate::{
    errors::Error,
    observability::http_tracing::create_operation_span,
    openapi::defaults::is_default_gateway_route,
    storage::{
        CreateRouteRepositoryRequest, RouteData, RouteRepository, UpdateRouteRepositoryRequest,
    },
    xds::{route::RouteConfig, XdsState},
};
use opentelemetry::{
    trace::{Span, SpanKind},
    KeyValue,
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
        use opentelemetry::trace::{FutureExt, TraceContextExt};

        let mut span = create_operation_span("route_service.create_route", SpanKind::Internal);
        span.set_attribute(KeyValue::new("route.name", name.clone()));
        span.set_attribute(KeyValue::new("route.path_prefix", path_prefix.clone()));
        span.set_attribute(KeyValue::new("route.cluster", cluster_summary.clone()));

        let cx = opentelemetry::Context::current().with_span(span);

        async move {
            let repository = self.repository()?;

            let request = CreateRouteRepositoryRequest {
                name: name.clone(),
                path_prefix,
                cluster_name: cluster_summary,
                configuration: config,
                team: None, // Native API routes don't have team assignment by default
            };

            let mut db_span = create_operation_span("db.route.insert", SpanKind::Client);
            db_span.set_attribute(KeyValue::new("db.operation", "INSERT"));
            db_span.set_attribute(KeyValue::new("db.table", "routes"));
            let created = repository.create(request).await?;
            drop(db_span);

            info!(
                route_id = %created.id,
                route_name = %created.name,
                "Route created"
            );

            let mut xds_span = create_operation_span("xds.refresh_routes", SpanKind::Internal);
            xds_span.set_attribute(KeyValue::new("route.id", created.id.to_string()));
            self.refresh_xds().await?;
            drop(xds_span);

            Ok(created)
        }
        .with_context(cx)
        .await
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
        use opentelemetry::trace::{FutureExt, TraceContextExt};

        let mut span = create_operation_span("route_service.update_route", SpanKind::Internal);
        span.set_attribute(KeyValue::new("route.name", name.to_string()));
        span.set_attribute(KeyValue::new("route.path_prefix", path_prefix.clone()));
        span.set_attribute(KeyValue::new("route.cluster", cluster_summary.clone()));

        let cx = opentelemetry::Context::current().with_span(span);

        async move {
            let repository = self.repository()?;
            let existing = repository.get_by_name(name).await?;

            let update_request = UpdateRouteRepositoryRequest {
                path_prefix: Some(path_prefix),
                cluster_name: Some(cluster_summary),
                configuration: Some(config),
                team: None, // Don't modify team on update unless explicitly set
            };

            let mut db_span = create_operation_span("db.route.update", SpanKind::Client);
            db_span.set_attribute(KeyValue::new("db.operation", "UPDATE"));
            db_span.set_attribute(KeyValue::new("db.table", "routes"));
            let updated = repository.update(&existing.id, update_request).await?;
            drop(db_span);

            info!(
                route_id = %updated.id,
                route_name = %updated.name,
                "Route updated"
            );

            let mut xds_span = create_operation_span("xds.refresh_routes", SpanKind::Internal);
            xds_span.set_attribute(KeyValue::new("route.id", updated.id.to_string()));
            self.refresh_xds().await?;
            drop(xds_span);

            Ok(updated)
        }
        .with_context(cx)
        .await
    }

    /// Delete a route by name
    pub async fn delete_route(&self, name: &str) -> Result<(), Error> {
        use opentelemetry::trace::{FutureExt, TraceContextExt};

        if is_default_gateway_route(name) {
            return Err(Error::validation("The default gateway route cannot be deleted"));
        }

        let mut span = create_operation_span("route_service.delete_route", SpanKind::Internal);
        span.set_attribute(KeyValue::new("route.name", name.to_string()));

        let cx = opentelemetry::Context::current().with_span(span);

        async move {
            let repository = self.repository()?;
            let existing = repository.get_by_name(name).await?;

            let mut db_span = create_operation_span("db.route.delete", SpanKind::Client);
            db_span.set_attribute(KeyValue::new("db.operation", "DELETE"));
            db_span.set_attribute(KeyValue::new("db.table", "routes"));
            db_span.set_attribute(KeyValue::new("route.id", existing.id.to_string()));
            repository.delete(&existing.id).await?;
            drop(db_span);

            info!(
                route_id = %existing.id,
                route_name = %existing.name,
                "Route deleted"
            );

            let mut xds_span = create_operation_span("xds.refresh_routes", SpanKind::Internal);
            xds_span.set_attribute(KeyValue::new("route.id", existing.id.to_string()));
            self.refresh_xds().await?;
            drop(xds_span);

            Ok(())
        }
        .with_context(cx)
        .await
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
