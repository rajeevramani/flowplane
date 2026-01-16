//! Filter business logic service
//!
//! This module contains the business logic for filter operations,
//! separated from HTTP concerns.

use std::sync::Arc;
use tracing::{debug, error, info};

use crate::{
    api::handlers::filters::{ClusterCreationConfig, ClusterMode},
    domain::{
        can_filter_type_attach_to, AttachmentPoint, FilterConfig, FilterId, FilterType, ListenerId,
        RouteConfigId, RouteId, VirtualHostId,
    },
    errors::Error,
    observability::http_tracing::create_operation_span,
    storage::{
        ClusterRepository, CreateClusterRequest, CreateFilterRequest, FilterData, FilterRepository,
        UpdateFilterRequest,
    },
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
        filter_type: String,
        description: Option<String>,
        config: FilterConfig,
        team: String,
    ) -> Result<FilterData, Error> {
        use opentelemetry::trace::{FutureExt, TraceContextExt};

        let mut span = create_operation_span("filter_service.create_filter", SpanKind::Internal);
        span.set_attribute(KeyValue::new("filter.name", name.clone()));
        span.set_attribute(KeyValue::new("filter.type", filter_type.clone()));
        span.set_attribute(KeyValue::new("filter.team", team.clone()));

        let cx = opentelemetry::Context::current().with_span(span);

        async move {
            let repository = self.repository()?;

            // Validate config matches type
            // For custom filters, the type is embedded in the config
            if config.is_custom() {
                if config.filter_type_str() != filter_type {
                    return Err(Error::validation(format!(
                        "Filter type '{}' does not match config type '{}'",
                        filter_type,
                        config.filter_type_str()
                    )));
                }
            } else {
                // For built-in types, validate the match
                match (filter_type.as_str(), &config) {
                    ("header_mutation", FilterConfig::HeaderMutation(_)) => {}
                    ("jwt_auth", FilterConfig::JwtAuth(_)) => {}
                    ("local_rate_limit", FilterConfig::LocalRateLimit(_)) => {}
                    ("custom_response", FilterConfig::CustomResponse(_)) => {}
                    ("mcp", FilterConfig::Mcp(_)) => {}
                    ("cors", FilterConfig::Cors(_)) => {}
                    ("compressor", FilterConfig::Compressor(_)) => {}
                    ("ext_authz", FilterConfig::ExtAuthz(_)) => {}
                    ("rbac", FilterConfig::Rbac(_)) => {}
                    ("oauth2", FilterConfig::OAuth2(_)) => {}
                    _ => {
                        return Err(Error::validation("Filter type and configuration do not match"))
                    }
                }
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
                filter_type: filter_type.clone(),
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
                    (FilterType::JwtAuth, FilterConfig::JwtAuth(_)) => {}
                    (FilterType::LocalRateLimit, FilterConfig::LocalRateLimit(_)) => {}
                    (FilterType::CustomResponse, FilterConfig::CustomResponse(_)) => {}
                    (FilterType::Mcp, FilterConfig::Mcp(_)) => {}
                    (FilterType::Cors, FilterConfig::Cors(_)) => {}
                    (FilterType::Compressor, FilterConfig::Compressor(_)) => {}
                    (FilterType::ExtAuthz, FilterConfig::ExtAuthz(_)) => {}
                    (FilterType::Rbac, FilterConfig::Rbac(_)) => {}
                    (FilterType::OAuth2, FilterConfig::OAuth2(_)) => {}
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

    /// Configure a filter for a route config scope
    ///
    /// Validates that the filter type supports route-level attachment before proceeding.
    /// Note: User must explicitly install the filter on listeners before configuring scope.
    pub async fn attach_filter_to_route_config(
        &self,
        route_config_id: &RouteConfigId,
        filter_id: &FilterId,
        order: Option<i64>,
        settings: Option<serde_json::Value>,
    ) -> Result<(), Error> {
        use opentelemetry::trace::{FutureExt, TraceContextExt};

        let mut span = create_operation_span(
            "filter_service.attach_filter_to_route_config",
            SpanKind::Internal,
        );
        span.set_attribute(KeyValue::new("route_config.id", route_config_id.to_string()));
        span.set_attribute(KeyValue::new("filter.id", filter_id.to_string()));

        let cx = opentelemetry::Context::current().with_span(span);

        async move {
            let repository = self.repository()?;

            // Verify filter exists and get its type
            let filter = repository.get_by_id(filter_id).await?;

            // Validate filter type can attach to routes (supports both built-in and custom types)
            can_filter_type_attach_to(&filter.filter_type, AttachmentPoint::Route)
                .map_err(Error::validation)?;

            // Determine order (default: append to end using MAX + 1)
            let order = match order {
                Some(o) => o,
                None => repository.get_next_route_config_filter_order(route_config_id).await?,
            };

            let mut db_span =
                create_operation_span("db.route_config_filters.upsert", SpanKind::Client);
            db_span.set_attribute(KeyValue::new("db.operation", "UPSERT"));
            db_span.set_attribute(KeyValue::new("db.table", "route_config_filters"));
            repository.upsert_to_route_config(route_config_id, filter_id, order, settings).await?;
            drop(db_span);

            info!(
                route_config_id = %route_config_id,
                filter_id = %filter_id,
                order = %order,
                "Filter configured for route config scope"
            );

            let mut xds_span = create_operation_span("xds.refresh_routes", SpanKind::Internal);
            xds_span.set_attribute(KeyValue::new("route_config.id", route_config_id.to_string()));
            xds_span.set_attribute(KeyValue::new("filter.id", filter_id.to_string()));
            self.refresh_xds().await?;
            drop(xds_span);

            Ok(())
        }
        .with_context(cx)
        .await
    }

    /// Remove a filter configuration from a route config scope
    pub async fn detach_filter_from_route_config(
        &self,
        route_config_id: &RouteConfigId,
        filter_id: &FilterId,
    ) -> Result<(), Error> {
        use opentelemetry::trace::{FutureExt, TraceContextExt};

        let mut span = create_operation_span(
            "filter_service.detach_filter_from_route_config",
            SpanKind::Internal,
        );
        span.set_attribute(KeyValue::new("route_config.id", route_config_id.to_string()));
        span.set_attribute(KeyValue::new("filter.id", filter_id.to_string()));

        let cx = opentelemetry::Context::current().with_span(span);

        async move {
            let repository = self.repository()?;

            let mut db_span =
                create_operation_span("db.route_config_filters.delete", SpanKind::Client);
            db_span.set_attribute(KeyValue::new("db.operation", "DELETE"));
            db_span.set_attribute(KeyValue::new("db.table", "route_config_filters"));
            repository.detach_from_route_config(route_config_id, filter_id).await?;
            drop(db_span);

            info!(
                route_config_id = %route_config_id,
                filter_id = %filter_id,
                "Filter configuration removed from route config scope"
            );

            let mut xds_span = create_operation_span("xds.refresh_routes", SpanKind::Internal);
            xds_span.set_attribute(KeyValue::new("route_config.id", route_config_id.to_string()));
            xds_span.set_attribute(KeyValue::new("filter.id", filter_id.to_string()));
            self.refresh_xds().await?;
            drop(xds_span);

            Ok(())
        }
        .with_context(cx)
        .await
    }

    /// List all filters attached to a route config
    pub async fn list_route_config_filters(
        &self,
        route_config_id: &RouteConfigId,
    ) -> Result<Vec<FilterData>, Error> {
        let repository = self.repository()?;
        repository.list_route_config_filters(route_config_id).await
    }

    /// List all route configs using a filter
    pub async fn list_filter_route_configs(
        &self,
        filter_id: &FilterId,
    ) -> Result<Vec<RouteConfigId>, Error> {
        let repository = self.repository()?;
        repository.list_filter_route_configs(filter_id).await
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

            // Validate filter type can attach to listeners (supports both built-in and custom types)
            can_filter_type_attach_to(&filter.filter_type, AttachmentPoint::Listener)
                .map_err(Error::validation)?;

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
    ///
    /// This refreshes both routes and listeners because:
    /// - Route-level filters need route refresh for typed_per_filter_config
    /// - Listener-attached filters (like JWT) need listener refresh for HCM injection
    async fn refresh_xds(&self) -> Result<(), Error> {
        // Refresh routes first (for typed_per_filter_config)
        self.xds_state.refresh_routes_from_repository().await.map_err(|err| {
            error!(error = %err, "Failed to refresh route xDS caches after filter operation");
            err
        })?;

        // Also refresh listeners (for JWT and other listener-attached filters)
        self.xds_state.refresh_listeners_from_repository().await.map_err(|err| {
            error!(error = %err, "Failed to refresh listener xDS caches after filter operation");
            err
        })
    }

    // =========================================================================
    // Virtual Host and Route Rule Filter Configuration (Hierarchical Filters)
    // =========================================================================

    /// Configure a filter for a virtual host scope
    ///
    /// Filters configured at the virtual host level apply to all route rules within that host.
    /// Note: User must explicitly install the filter on listeners before configuring scope.
    pub async fn attach_filter_to_virtual_host(
        &self,
        virtual_host_id: &VirtualHostId,
        filter_id: &FilterId,
        order: Option<i32>,
        settings: Option<serde_json::Value>,
    ) -> Result<(), Error> {
        use opentelemetry::trace::{FutureExt, TraceContextExt};

        let mut span =
            create_operation_span("filter_service.attach_filter_to_vh", SpanKind::Internal);
        span.set_attribute(KeyValue::new("virtual_host.id", virtual_host_id.to_string()));
        span.set_attribute(KeyValue::new("filter.id", filter_id.to_string()));

        let cx = opentelemetry::Context::current().with_span(span);

        async move {
            let filter_repo = self.repository()?;
            let vh_filter_repo = self.virtual_host_filter_repository()?;

            // Verify filter exists and get its type
            let filter = filter_repo.get_by_id(filter_id).await?;

            // Validate filter type can attach to routes (supports both built-in and custom types)
            // Virtual hosts use Route attachment point
            can_filter_type_attach_to(&filter.filter_type, AttachmentPoint::Route)
                .map_err(Error::validation)?;

            // Determine order
            let order = match order {
                Some(o) => o,
                None => vh_filter_repo.get_next_order(virtual_host_id).await?,
            };

            vh_filter_repo.upsert(virtual_host_id, filter_id, order, settings).await?;

            info!(
                virtual_host_id = %virtual_host_id,
                filter_id = %filter_id,
                order = %order,
                "Filter configured for virtual host scope"
            );

            self.refresh_xds().await?;

            Ok(())
        }
        .with_context(cx)
        .await
    }

    /// Detach a filter from a virtual host
    pub async fn detach_filter_from_virtual_host(
        &self,
        virtual_host_id: &VirtualHostId,
        filter_id: &FilterId,
    ) -> Result<(), Error> {
        use opentelemetry::trace::{FutureExt, TraceContextExt};

        let mut span =
            create_operation_span("filter_service.detach_filter_from_vh", SpanKind::Internal);
        span.set_attribute(KeyValue::new("virtual_host.id", virtual_host_id.to_string()));
        span.set_attribute(KeyValue::new("filter.id", filter_id.to_string()));

        let cx = opentelemetry::Context::current().with_span(span);

        async move {
            let vh_filter_repo = self.virtual_host_filter_repository()?;

            vh_filter_repo.detach(virtual_host_id, filter_id).await?;

            info!(
                virtual_host_id = %virtual_host_id,
                filter_id = %filter_id,
                "Filter detached from virtual host"
            );

            self.refresh_xds().await?;

            Ok(())
        }
        .with_context(cx)
        .await
    }

    /// List all filters attached to a virtual host
    pub async fn list_virtual_host_filters(
        &self,
        virtual_host_id: &VirtualHostId,
    ) -> Result<Vec<FilterData>, Error> {
        let vh_filter_repo = self.virtual_host_filter_repository()?;
        let filter_repo = self.repository()?;

        let attachments = vh_filter_repo.list_by_virtual_host(virtual_host_id).await?;

        let mut filters = Vec::with_capacity(attachments.len());
        for attachment in attachments {
            let filter = filter_repo.get_by_id(&attachment.filter_id).await?;
            filters.push(filter);
        }

        Ok(filters)
    }

    /// Configure a filter for a route scope
    ///
    /// Filters configured at the route level apply only to that specific route,
    /// overriding filters configured at the virtual host or route config level.
    /// Note: User must explicitly install the filter on listeners before configuring scope.
    pub async fn attach_filter_to_route(
        &self,
        route_id: &RouteId,
        filter_id: &FilterId,
        order: Option<i32>,
        settings: Option<serde_json::Value>,
    ) -> Result<(), Error> {
        use opentelemetry::trace::{FutureExt, TraceContextExt};

        let mut span =
            create_operation_span("filter_service.attach_filter_to_route", SpanKind::Internal);
        span.set_attribute(KeyValue::new("route.id", route_id.to_string()));
        span.set_attribute(KeyValue::new("filter.id", filter_id.to_string()));

        let cx = opentelemetry::Context::current().with_span(span);

        async move {
            let filter_repo = self.repository()?;
            let route_filter_repo = self.route_filter_repository()?;

            // Verify filter exists and get its type
            let filter = filter_repo.get_by_id(filter_id).await?;

            // Validate filter type can attach to routes (supports both built-in and custom types)
            can_filter_type_attach_to(&filter.filter_type, AttachmentPoint::Route)
                .map_err(Error::validation)?;

            // Determine order
            let order = match order {
                Some(o) => o,
                None => route_filter_repo.get_next_order(route_id).await?,
            };

            route_filter_repo.upsert(route_id, filter_id, order, settings).await?;

            info!(
                route_id = %route_id,
                filter_id = %filter_id,
                order = %order,
                "Filter configured for route scope"
            );

            self.refresh_xds().await?;

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
            let route_filter_repo = self.route_filter_repository()?;

            route_filter_repo.detach(route_id, filter_id).await?;

            info!(
                route_id = %route_id,
                filter_id = %filter_id,
                "Filter detached from route"
            );

            self.refresh_xds().await?;

            Ok(())
        }
        .with_context(cx)
        .await
    }

    /// List all filters attached to a route
    pub async fn list_route_filters(&self, route_id: &RouteId) -> Result<Vec<FilterData>, Error> {
        let route_filter_repo = self.route_filter_repository()?;
        let filter_repo = self.repository()?;

        let attachments = route_filter_repo.list_by_route(route_id).await?;

        let mut filters = Vec::with_capacity(attachments.len());
        for attachment in attachments {
            let filter = filter_repo.get_by_id(&attachment.filter_id).await?;
            filters.push(filter);
        }

        Ok(filters)
    }

    // Helper methods for hierarchical repositories

    fn virtual_host_filter_repository(
        &self,
    ) -> Result<crate::storage::VirtualHostFilterRepository, Error> {
        self.xds_state
            .virtual_host_filter_repository
            .as_ref()
            .cloned()
            .ok_or_else(|| Error::internal("Virtual host filter repository not configured"))
    }

    fn route_filter_repository(&self) -> Result<crate::storage::RouteFilterRepository, Error> {
        self.xds_state
            .route_filter_repository
            .as_ref()
            .cloned()
            .ok_or_else(|| Error::internal("Route filter repository not configured"))
    }

    /// Get the cluster repository
    fn cluster_repository(&self) -> Result<ClusterRepository, Error> {
        self.xds_state
            .cluster_repository
            .as_ref()
            .cloned()
            .ok_or_else(|| Error::internal("Cluster repository not configured"))
    }

    /// Handle cluster creation or validation based on ClusterCreationConfig
    ///
    /// When mode is "create", creates a new cluster with the provided spec.
    /// When mode is "reuse", validates that the cluster exists.
    ///
    /// Returns the cluster name on success.
    pub async fn handle_cluster_config(
        &self,
        cluster_config: &ClusterCreationConfig,
        filter_team: &str,
    ) -> Result<String, Error> {
        let cluster_repo = self.cluster_repository()?;
        let cluster_name = cluster_config.cluster_name.clone();

        match cluster_config.mode {
            ClusterMode::Create => {
                // Validate that cluster doesn't already exist
                if cluster_repo.exists_by_name(&cluster_name).await? {
                    return Err(Error::conflict(
                        format!("Cluster '{}' already exists. Use mode 'reuse' to reference an existing cluster.", cluster_name),
                        "cluster",
                    ));
                }

                // Get the cluster spec (already validated in request validation)
                let cluster_spec = cluster_config.cluster_spec.as_ref().ok_or_else(|| {
                    Error::validation("cluster_spec is required when mode is 'create'")
                })?;

                let service_name = cluster_config.service_name.as_ref().ok_or_else(|| {
                    Error::validation("service_name is required when mode is 'create'")
                })?;

                // Determine team for the cluster
                let team = cluster_config.team.as_ref().unwrap_or(&filter_team.to_string()).clone();

                // Serialize cluster spec
                let configuration = cluster_spec.to_value()?;

                // Create the cluster
                let create_request = CreateClusterRequest {
                    name: cluster_name.clone(),
                    service_name: service_name.clone(),
                    configuration,
                    team: Some(team.clone()),
                    import_id: None,
                };

                let mut db_span = create_operation_span("db.cluster.insert", SpanKind::Client);
                db_span.set_attribute(KeyValue::new("db.operation", "INSERT"));
                db_span.set_attribute(KeyValue::new("db.table", "clusters"));
                let created = cluster_repo.create(create_request).await?;
                drop(db_span);

                info!(
                    cluster_id = %created.id,
                    cluster_name = %created.name,
                    team = %team,
                    "Cluster created for filter"
                );

                // Refresh cluster xDS cache
                self.xds_state.refresh_clusters_from_repository().await.map_err(|err| {
                    error!(error = %err, "Failed to refresh cluster xDS caches");
                    err
                })?;

                Ok(cluster_name)
            }
            ClusterMode::Reuse => {
                // Validate that cluster exists
                if !cluster_repo.exists_by_name(&cluster_name).await? {
                    return Err(Error::not_found(
                        "cluster",
                        format!(
                            "'{}' not found. Create it first or use mode 'create'.",
                            cluster_name
                        ),
                    ));
                }

                debug!(
                    cluster_name = %cluster_name,
                    "Reusing existing cluster for filter"
                );

                Ok(cluster_name)
            }
        }
    }

    /// Create a filter with optional cluster creation
    ///
    /// This is a convenience method that handles cluster creation before filter creation.
    /// If cluster_config is provided with mode "create", the cluster is created first.
    /// If mode is "reuse", the cluster existence is validated.
    pub async fn create_filter_with_cluster(
        &self,
        name: String,
        filter_type: String,
        description: Option<String>,
        config: FilterConfig,
        team: String,
        cluster_config: Option<ClusterCreationConfig>,
    ) -> Result<FilterData, Error> {
        // Handle cluster creation/validation if config provided
        if let Some(ref cc) = cluster_config {
            self.handle_cluster_config(cc, &team).await?;
        }

        // Create the filter
        self.create_filter(name, filter_type, description, config, team).await
    }
}
