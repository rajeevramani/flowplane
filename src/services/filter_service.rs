//! Filter business logic service
//!
//! This module contains the business logic for filter operations,
//! separated from HTTP concerns.

use std::sync::Arc;
use tracing::{debug, error, info, warn};

use crate::{
    domain::{AttachmentPoint, FilterConfig, FilterId, FilterType, ListenerId, RouteId},
    errors::Error,
    observability::http_tracing::create_operation_span,
    services::listener_filter_chain::{
        add_http_filter_before_router, listener_has_http_filter, remove_http_filter_from_listener,
    },
    storage::{
        CreateFilterRequest, FilterData, FilterRepository, ListenerAutoFilterRepository,
        ListenerRepository, RouteRepository, UpdateFilterRequest,
    },
    xds::{listener::ListenerConfig, XdsState},
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

    /// Get the route repository
    fn route_repository(&self) -> Result<RouteRepository, Error> {
        self.xds_state
            .route_repository
            .as_ref()
            .cloned()
            .ok_or_else(|| Error::internal("Route repository not configured"))
    }

    /// Get the listener repository
    fn listener_repository(&self) -> Result<ListenerRepository, Error> {
        self.xds_state
            .listener_repository
            .as_ref()
            .cloned()
            .ok_or_else(|| Error::internal("Listener repository not configured"))
    }

    /// Get the listener auto-filter repository
    fn auto_filter_repository(&self) -> Result<ListenerAutoFilterRepository, Error> {
        self.xds_state
            .listener_auto_filter_repository
            .as_ref()
            .cloned()
            .ok_or_else(|| Error::internal("Listener auto-filter repository not configured"))
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
    ///
    /// Fails if the filter is attached to any routes or listeners.
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
            let listeners = repository.list_filter_listeners(id).await?;

            if !routes.is_empty() || !listeners.is_empty() {
                let mut parts = Vec::new();
                if !routes.is_empty() {
                    parts.push(format!("{} route(s)", routes.len()));
                }
                if !listeners.is_empty() {
                    parts.push(format!("{} listener(s)", listeners.len()));
                }
                return Err(Error::conflict(
                    format!(
                        "Filter is attached to {}. Detach before deleting.",
                        parts.join(" and ")
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
    ///
    /// Validates that the filter type supports route-level attachment before proceeding.
    /// Also automatically adds the required HTTP filter to connected listeners.
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

            // Verify filter exists and get its type
            let filter = repository.get_by_id(filter_id).await?;

            // Validate filter type can attach to routes
            let filter_type: FilterType =
                serde_json::from_str(&format!("\"{}\"", filter.filter_type)).map_err(|err| {
                    Error::internal(format!("Failed to parse filter type: {}", err))
                })?;

            if !filter_type.can_attach_to(AttachmentPoint::Route) {
                return Err(Error::validation(format!(
                    "Filter type '{}' cannot be attached to routes. Valid attachment points: {}",
                    filter.filter_type,
                    filter_type.allowed_attachment_points_display()
                )));
            }

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

            // Auto-add HTTP filter to connected listeners
            self.ensure_listener_http_filters(route_id, filter_id, filter_type).await?;

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
    ///
    /// Also automatically removes the HTTP filter from connected listeners
    /// if no other routes need it.
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

            // Get filter type before detaching (needed for cleanup)
            let filter = repository.get_by_id(filter_id).await?;
            let filter_type: FilterType =
                serde_json::from_str(&format!("\"{}\"", filter.filter_type)).map_err(|err| {
                    Error::internal(format!("Failed to parse filter type: {}", err))
                })?;

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

            // Auto-remove HTTP filter from listeners if no longer needed
            self.cleanup_listener_http_filters(route_id, filter_id, filter_type).await?;

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

    // Listener filter attachment methods

    /// Attach a filter to a listener
    ///
    /// Validates that the filter type supports listener-level attachment before proceeding.
    /// Only JwtAuth, RateLimit, and ExtAuthz filters can attach to listeners.
    pub async fn attach_filter_to_listener(
        &self,
        listener_id: &ListenerId,
        filter_id: &FilterId,
        order: Option<i64>,
    ) -> Result<(), Error> {
        use opentelemetry::trace::{FutureExt, TraceContextExt};

        let mut span =
            create_operation_span("filter_service.attach_filter_to_listener", SpanKind::Internal);
        span.set_attribute(KeyValue::new("listener.id", listener_id.to_string()));
        span.set_attribute(KeyValue::new("filter.id", filter_id.to_string()));

        let cx = opentelemetry::Context::current().with_span(span);

        async move {
            let repository = self.repository()?;

            // Verify filter exists and get its type
            let filter = repository.get_by_id(filter_id).await?;

            // Validate filter type can attach to listeners
            let filter_type: FilterType =
                serde_json::from_str(&format!("\"{}\"", filter.filter_type)).map_err(|err| {
                    Error::internal(format!("Failed to parse filter type: {}", err))
                })?;

            if !filter_type.can_attach_to(AttachmentPoint::Listener) {
                return Err(Error::validation(format!(
                    "Filter type '{}' cannot be attached to listeners. Valid attachment points: {}",
                    filter.filter_type,
                    filter_type.allowed_attachment_points_display()
                )));
            }

            // Determine order (default: append to end)
            let order = match order {
                Some(o) => o,
                None => {
                    let existing = repository.list_listener_filters(listener_id).await?;
                    existing.len() as i64
                }
            };

            let mut db_span = create_operation_span("db.listener_filters.insert", SpanKind::Client);
            db_span.set_attribute(KeyValue::new("db.operation", "INSERT"));
            db_span.set_attribute(KeyValue::new("db.table", "listener_filters"));
            repository.attach_to_listener(listener_id, filter_id, order).await?;
            drop(db_span);

            info!(
                listener_id = %listener_id,
                filter_id = %filter_id,
                order = %order,
                "Filter attached to listener"
            );

            let mut xds_span = create_operation_span("xds.refresh_listeners", SpanKind::Internal);
            xds_span.set_attribute(KeyValue::new("listener.id", listener_id.to_string()));
            xds_span.set_attribute(KeyValue::new("filter.id", filter_id.to_string()));
            self.refresh_xds().await?;
            drop(xds_span);

            Ok(())
        }
        .with_context(cx)
        .await
    }

    /// Detach a filter from a listener
    pub async fn detach_filter_from_listener(
        &self,
        listener_id: &ListenerId,
        filter_id: &FilterId,
    ) -> Result<(), Error> {
        use opentelemetry::trace::{FutureExt, TraceContextExt};

        let mut span =
            create_operation_span("filter_service.detach_filter_from_listener", SpanKind::Internal);
        span.set_attribute(KeyValue::new("listener.id", listener_id.to_string()));
        span.set_attribute(KeyValue::new("filter.id", filter_id.to_string()));

        let cx = opentelemetry::Context::current().with_span(span);

        async move {
            let repository = self.repository()?;

            let mut db_span = create_operation_span("db.listener_filters.delete", SpanKind::Client);
            db_span.set_attribute(KeyValue::new("db.operation", "DELETE"));
            db_span.set_attribute(KeyValue::new("db.table", "listener_filters"));
            repository.detach_from_listener(listener_id, filter_id).await?;
            drop(db_span);

            info!(
                listener_id = %listener_id,
                filter_id = %filter_id,
                "Filter detached from listener"
            );

            let mut xds_span = create_operation_span("xds.refresh_listeners", SpanKind::Internal);
            xds_span.set_attribute(KeyValue::new("listener.id", listener_id.to_string()));
            xds_span.set_attribute(KeyValue::new("filter.id", filter_id.to_string()));
            self.refresh_xds().await?;
            drop(xds_span);

            Ok(())
        }
        .with_context(cx)
        .await
    }

    /// List all filters attached to a listener
    pub async fn list_listener_filters(
        &self,
        listener_id: &ListenerId,
    ) -> Result<Vec<FilterData>, Error> {
        let repository = self.repository()?;
        repository.list_listener_filters(listener_id).await
    }

    /// List all listeners using a filter
    pub async fn list_filter_listeners(
        &self,
        filter_id: &FilterId,
    ) -> Result<Vec<ListenerId>, Error> {
        let repository = self.repository()?;
        repository.list_filter_listeners(filter_id).await
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

    // =========================================================================
    // Automatic Listener Filter Chain Management
    // =========================================================================

    /// Ensure listeners referencing this route have the required HTTP filter
    ///
    /// When a filter is attached to a route, the corresponding HTTP filter must
    /// exist in the listener's filter chain for `typed_per_filter_config` to work.
    async fn ensure_listener_http_filters(
        &self,
        route_id: &RouteId,
        filter_id: &FilterId,
        filter_type: FilterType,
    ) -> Result<(), Error> {
        let route_repo = self.route_repository()?;
        let listener_repo = self.listener_repository()?;
        let auto_filter_repo = self.auto_filter_repository()?;

        // Get route to find its name (used as route_config_name in listeners)
        let route = route_repo.get_by_id(route_id).await?;

        // Find listeners using this route's name as route_config_name
        let listeners = listener_repo.find_by_route_config_name(&route.name, &[]).await?;

        if listeners.is_empty() {
            debug!(
                route_id = %route_id,
                route_name = %route.name,
                "No listeners found referencing this route"
            );
            return Ok(());
        }

        let http_filter_name = filter_type.http_filter_name();

        for listener in listeners {
            self.add_http_filter_to_listener(
                &listener.id,
                http_filter_name,
                filter_id,
                route_id,
                &listener_repo,
                &auto_filter_repo,
            )
            .await?;
        }

        Ok(())
    }

    /// Add HTTP filter to a listener's filter chain if not already present
    async fn add_http_filter_to_listener(
        &self,
        listener_id: &ListenerId,
        http_filter_name: &str,
        filter_id: &FilterId,
        route_id: &RouteId,
        listener_repo: &ListenerRepository,
        auto_filter_repo: &ListenerAutoFilterRepository,
    ) -> Result<(), Error> {
        // Check if already tracked (idempotent)
        if auto_filter_repo.exists(listener_id, http_filter_name, filter_id, route_id).await? {
            debug!(
                listener_id = %listener_id,
                http_filter_name = %http_filter_name,
                "Auto-filter tracking record already exists"
            );
            return Ok(());
        }

        // Load listener config
        let listener = listener_repo.get_by_id(listener_id).await?;
        let mut config: ListenerConfig =
            serde_json::from_str(&listener.configuration).map_err(|err| {
                Error::internal(format!("Failed to parse listener configuration: {}", err))
            })?;

        // Check if filter already in chain (manual or auto-added for other route)
        if !listener_has_http_filter(&config, http_filter_name) {
            // Add filter before router
            if add_http_filter_before_router(&mut config, http_filter_name) {
                // Save updated config
                let new_config = serde_json::to_string(&config).map_err(|err| {
                    Error::internal(format!("Failed to serialize listener configuration: {}", err))
                })?;
                listener_repo.update_configuration(listener_id, &new_config).await?;

                info!(
                    listener_id = %listener_id,
                    http_filter_name = %http_filter_name,
                    "Auto-added HTTP filter to listener"
                );
            }
        } else {
            debug!(
                listener_id = %listener_id,
                http_filter_name = %http_filter_name,
                "HTTP filter already exists in listener"
            );
        }

        // Track auto-added filter (even if filter already existed - for reference counting)
        auto_filter_repo.create(listener_id, http_filter_name, filter_id, route_id).await?;

        Ok(())
    }

    /// Remove HTTP filter from listeners if no other routes need it
    async fn cleanup_listener_http_filters(
        &self,
        route_id: &RouteId,
        filter_id: &FilterId,
        filter_type: FilterType,
    ) -> Result<(), Error> {
        let listener_repo = self.listener_repository()?;
        let auto_filter_repo = self.auto_filter_repository()?;

        let http_filter_name = filter_type.http_filter_name();

        // Get affected listeners (from tracking table)
        let affected = auto_filter_repo.get_by_source(filter_id, route_id).await?;

        if affected.is_empty() {
            debug!(
                filter_id = %filter_id,
                route_id = %route_id,
                "No auto-filter tracking records found for cleanup"
            );
            return Ok(());
        }

        // Remove tracking records for this source
        auto_filter_repo.delete_by_source(filter_id, route_id).await?;

        // For each affected listener, check if filter still needed
        for record in affected {
            let count = auto_filter_repo
                .count_by_listener_and_http_filter(&record.listener_id, http_filter_name)
                .await?;

            if count == 0 {
                // No other routes need this filter - remove from listener
                self.remove_http_filter_from_listener_config(
                    &record.listener_id,
                    http_filter_name,
                    &listener_repo,
                )
                .await?;
            } else {
                debug!(
                    listener_id = %record.listener_id,
                    http_filter_name = %http_filter_name,
                    remaining_references = count,
                    "HTTP filter still needed by other routes"
                );
            }
        }

        Ok(())
    }

    /// Remove HTTP filter from a listener's configuration
    async fn remove_http_filter_from_listener_config(
        &self,
        listener_id: &ListenerId,
        http_filter_name: &str,
        listener_repo: &ListenerRepository,
    ) -> Result<(), Error> {
        // Load listener config
        let listener = listener_repo.get_by_id(listener_id).await?;
        let mut config: ListenerConfig =
            serde_json::from_str(&listener.configuration).map_err(|err| {
                Error::internal(format!("Failed to parse listener configuration: {}", err))
            })?;

        // Remove filter
        if remove_http_filter_from_listener(&mut config, http_filter_name) {
            // Save updated config
            let new_config = serde_json::to_string(&config).map_err(|err| {
                Error::internal(format!("Failed to serialize listener configuration: {}", err))
            })?;
            listener_repo.update_configuration(listener_id, &new_config).await?;

            info!(
                listener_id = %listener_id,
                http_filter_name = %http_filter_name,
                "Auto-removed HTTP filter from listener"
            );
        } else {
            warn!(
                listener_id = %listener_id,
                http_filter_name = %http_filter_name,
                "HTTP filter not found in listener during cleanup"
            );
        }

        Ok(())
    }
}
