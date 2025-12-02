//! Filter business logic service
//!
//! This module contains the business logic for filter operations,
//! separated from HTTP concerns.

use std::sync::Arc;
use tracing::{error, info};

use crate::{
    domain::{FilterConfig, FilterId, FilterType, RouteId},
    errors::Error,
    observability::http_tracing::create_operation_span,
    storage::{CreateFilterRequest, FilterData, FilterRepository, UpdateFilterRequest},
    xds::XdsState,
};
use opentelemetry::{
    trace::{Span, SpanKind},
    KeyValue,
};

/// Service for managing filter business logic
pub struct FilterService {
    xds_state: Arc<XdsState>,
}

impl FilterService {
    /// Create a new filter service
    pub fn new(xds_state: Arc<XdsState>) -> Self {
        Self { xds_state }
    }

    /// Get the filter repository
    fn repository(&self) -> Result<FilterRepository, Error> {
        self.xds_state
            .filter_repository
            .as_ref()
            .cloned()
            .ok_or_else(|| Error::internal("Filter repository not configured"))
    }

    /// Create a new filter
    pub async fn create_filter(
        &self,
        name: String,
        filter_type: FilterType,
        description: Option<String>,
        config: FilterConfig,
        team: String,
    ) -> Result<FilterData, Error> {
        use opentelemetry::trace::{FutureExt, TraceContextExt};

        let mut span = create_operation_span("filter_service.create_filter", SpanKind::Internal);
        span.set_attribute(KeyValue::new("filter.name", name.clone()));
        span.set_attribute(KeyValue::new("filter.type", filter_type.to_string()));
        span.set_attribute(KeyValue::new("filter.team", team.clone()));

        let cx = opentelemetry::Context::current().with_span(span);

        async move {
            let repository = self.repository()?;

            // Validate config matches type
            match (&filter_type, &config) {
                (FilterType::HeaderMutation, FilterConfig::HeaderMutation(_)) => {}
                _ => return Err(Error::validation("Filter type and configuration do not match")),
            }

            // Check if filter name already exists for this team
            if repository.exists_by_name(&team, &name).await? {
                return Err(Error::conflict(
                    format!("Filter with name '{}' already exists for team '{}'", name, team),
                    "filter",
                ));
            }

            // Serialize config
            let configuration = serde_json::to_string(&config).map_err(|err| {
                Error::internal(format!("Failed to serialize filter configuration: {}", err))
            })?;

            let request = CreateFilterRequest {
                name: name.clone(),
                filter_type: filter_type.to_string(),
                description,
                configuration,
                team: team.clone(),
            };

            let mut db_span = create_operation_span("db.filter.insert", SpanKind::Client);
            db_span.set_attribute(KeyValue::new("db.operation", "INSERT"));
            db_span.set_attribute(KeyValue::new("db.table", "filters"));
            let created = repository.create(request).await?;
            drop(db_span);

            info!(
                filter_id = %created.id,
                filter_name = %created.name,
                filter_type = %created.filter_type,
                "Filter created"
            );

            let mut xds_span = create_operation_span("xds.refresh_routes", SpanKind::Internal);
            xds_span.set_attribute(KeyValue::new("filter.id", created.id.to_string()));
            self.refresh_xds().await?;
            drop(xds_span);

            Ok(created)
        }
        .with_context(cx)
        .await
    }

    /// List all filters for given teams
    pub async fn list_filters(
        &self,
        teams: &[String],
        limit: Option<i32>,
        offset: Option<i32>,
    ) -> Result<Vec<FilterData>, Error> {
        let repository = self.repository()?;
        repository.list_by_teams(teams, limit, offset).await
    }

    /// Get a filter by ID
    pub async fn get_filter(&self, id: &FilterId) -> Result<FilterData, Error> {
        let repository = self.repository()?;
        repository.get_by_id(id).await
    }

    /// Update an existing filter
    pub async fn update_filter(
        &self,
        id: &FilterId,
        name: Option<String>,
        description: Option<String>,
        config: Option<FilterConfig>,
    ) -> Result<FilterData, Error> {
        use opentelemetry::trace::{FutureExt, TraceContextExt};

        let mut span = create_operation_span("filter_service.update_filter", SpanKind::Internal);
        span.set_attribute(KeyValue::new("filter.id", id.to_string()));

        let cx = opentelemetry::Context::current().with_span(span);

        async move {
            let repository = self.repository()?;
            let existing = repository.get_by_id(id).await?;

            // If name is being changed, check for conflicts
            if let Some(ref new_name) = name {
                if new_name != &existing.name
                    && repository.exists_by_name(&existing.team, new_name).await?
                {
                    return Err(Error::conflict(
                        format!(
                            "Filter with name '{}' already exists for team '{}'",
                            new_name, existing.team
                        ),
                        "filter",
                    ));
                }
            }

            // Serialize config if provided
            let configuration = if let Some(config) = config {
                // Validate config matches existing type
                let existing_type: FilterType = serde_json::from_str(&format!(
                    "\"{}\"",
                    existing.filter_type
                ))
                .map_err(|err| {
                    Error::internal(format!("Failed to parse existing filter type: {}", err))
                })?;

                match (&existing_type, &config) {
                    (FilterType::HeaderMutation, FilterConfig::HeaderMutation(_)) => {}
                    _ => {
                        return Err(Error::validation("Cannot change filter type, config mismatch"))
                    }
                }

                Some(serde_json::to_string(&config).map_err(|err| {
                    Error::internal(format!("Failed to serialize filter configuration: {}", err))
                })?)
            } else {
                None
            };

            let update_request = UpdateFilterRequest { name, description, configuration };

            let mut db_span = create_operation_span("db.filter.update", SpanKind::Client);
            db_span.set_attribute(KeyValue::new("db.operation", "UPDATE"));
            db_span.set_attribute(KeyValue::new("db.table", "filters"));
            let updated = repository.update(id, update_request).await?;
            drop(db_span);

            info!(
                filter_id = %updated.id,
                filter_name = %updated.name,
                "Filter updated"
            );

            let mut xds_span = create_operation_span("xds.refresh_routes", SpanKind::Internal);
            xds_span.set_attribute(KeyValue::new("filter.id", updated.id.to_string()));
            self.refresh_xds().await?;
            drop(xds_span);

            Ok(updated)
        }
        .with_context(cx)
        .await
    }

    /// Delete a filter by ID
    pub async fn delete_filter(&self, id: &FilterId) -> Result<(), Error> {
        use opentelemetry::trace::{FutureExt, TraceContextExt};

        let mut span = create_operation_span("filter_service.delete_filter", SpanKind::Internal);
        span.set_attribute(KeyValue::new("filter.id", id.to_string()));

        let cx = opentelemetry::Context::current().with_span(span);

        async move {
            let repository = self.repository()?;
            let existing = repository.get_by_id(id).await?;

            // Check if filter is attached to any routes
            let routes = repository.list_filter_routes(id).await?;
            if !routes.is_empty() {
                return Err(Error::conflict(
                    format!(
                        "Filter is attached to {} route(s). Detach before deleting.",
                        routes.len()
                    ),
                    "filter",
                ));
            }

            let mut db_span = create_operation_span("db.filter.delete", SpanKind::Client);
            db_span.set_attribute(KeyValue::new("db.operation", "DELETE"));
            db_span.set_attribute(KeyValue::new("db.table", "filters"));
            db_span.set_attribute(KeyValue::new("filter.id", existing.id.to_string()));
            repository.delete(id).await?;
            drop(db_span);

            info!(
                filter_id = %existing.id,
                filter_name = %existing.name,
                "Filter deleted"
            );

            let mut xds_span = create_operation_span("xds.refresh_routes", SpanKind::Internal);
            xds_span.set_attribute(KeyValue::new("filter.id", existing.id.to_string()));
            self.refresh_xds().await?;
            drop(xds_span);

            Ok(())
        }
        .with_context(cx)
        .await
    }

    /// Attach a filter to a route
    pub async fn attach_filter_to_route(
        &self,
        route_id: &RouteId,
        filter_id: &FilterId,
        order: Option<i64>,
    ) -> Result<(), Error> {
        use opentelemetry::trace::{FutureExt, TraceContextExt};

        let mut span =
            create_operation_span("filter_service.attach_filter_to_route", SpanKind::Internal);
        span.set_attribute(KeyValue::new("route.id", route_id.to_string()));
        span.set_attribute(KeyValue::new("filter.id", filter_id.to_string()));

        let cx = opentelemetry::Context::current().with_span(span);

        async move {
            let repository = self.repository()?;

            // Verify filter exists
            repository.get_by_id(filter_id).await?;

            // Determine order (default: append to end)
            let order = match order {
                Some(o) => o,
                None => {
                    let existing = repository.list_route_filters(route_id).await?;
                    existing.len() as i64
                }
            };

            let mut db_span = create_operation_span("db.route_filters.insert", SpanKind::Client);
            db_span.set_attribute(KeyValue::new("db.operation", "INSERT"));
            db_span.set_attribute(KeyValue::new("db.table", "route_filters"));
            repository.attach_to_route(route_id, filter_id, order).await?;
            drop(db_span);

            info!(
                route_id = %route_id,
                filter_id = %filter_id,
                order = %order,
                "Filter attached to route"
            );

            let mut xds_span = create_operation_span("xds.refresh_routes", SpanKind::Internal);
            xds_span.set_attribute(KeyValue::new("route.id", route_id.to_string()));
            xds_span.set_attribute(KeyValue::new("filter.id", filter_id.to_string()));
            self.refresh_xds().await?;
            drop(xds_span);

            Ok(())
        }
        .with_context(cx)
        .await
    }

    /// Detach a filter from a route
    pub async fn detach_filter_from_route(
        &self,
        route_id: &RouteId,
        filter_id: &FilterId,
    ) -> Result<(), Error> {
        use opentelemetry::trace::{FutureExt, TraceContextExt};

        let mut span =
            create_operation_span("filter_service.detach_filter_from_route", SpanKind::Internal);
        span.set_attribute(KeyValue::new("route.id", route_id.to_string()));
        span.set_attribute(KeyValue::new("filter.id", filter_id.to_string()));

        let cx = opentelemetry::Context::current().with_span(span);

        async move {
            let repository = self.repository()?;

            let mut db_span = create_operation_span("db.route_filters.delete", SpanKind::Client);
            db_span.set_attribute(KeyValue::new("db.operation", "DELETE"));
            db_span.set_attribute(KeyValue::new("db.table", "route_filters"));
            repository.detach_from_route(route_id, filter_id).await?;
            drop(db_span);

            info!(
                route_id = %route_id,
                filter_id = %filter_id,
                "Filter detached from route"
            );

            let mut xds_span = create_operation_span("xds.refresh_routes", SpanKind::Internal);
            xds_span.set_attribute(KeyValue::new("route.id", route_id.to_string()));
            xds_span.set_attribute(KeyValue::new("filter.id", filter_id.to_string()));
            self.refresh_xds().await?;
            drop(xds_span);

            Ok(())
        }
        .with_context(cx)
        .await
    }

    /// List all filters attached to a route
    pub async fn list_route_filters(&self, route_id: &RouteId) -> Result<Vec<FilterData>, Error> {
        let repository = self.repository()?;
        repository.list_route_filters(route_id).await
    }

    /// List all routes using a filter
    pub async fn list_filter_routes(&self, filter_id: &FilterId) -> Result<Vec<RouteId>, Error> {
        let repository = self.repository()?;
        repository.list_filter_routes(filter_id).await
    }

    /// Parse filter configuration from stored JSON
    pub fn parse_config(&self, data: &FilterData) -> Result<FilterConfig, Error> {
        serde_json::from_str(&data.configuration).map_err(|err| {
            Error::internal(format!("Failed to parse stored filter configuration: {}", err))
        })
    }

    /// Refresh xDS caches after filter changes
    async fn refresh_xds(&self) -> Result<(), Error> {
        self.xds_state.refresh_routes_from_repository().await.map_err(|err| {
            error!(error = %err, "Failed to refresh xDS caches after filter operation");
            err
        })
    }
}
