//! Listener business logic service
//!
//! This module contains the business logic for listener operations,
//! separated from HTTP concerns.

use std::sync::Arc;
use tracing::{error, info};

use crate::{
    errors::Error,
    observability::http_tracing::create_operation_span,
    openapi::defaults::is_default_gateway_listener,
    storage::{CreateListenerRequest, ListenerData, ListenerRepository, UpdateListenerRequest},
    xds::{listener::ListenerConfig, XdsState},
};
use opentelemetry::{
    trace::{Span, SpanKind},
    KeyValue,
};

/// Service for managing listener business logic
pub struct ListenerService {
    xds_state: Arc<XdsState>,
}

impl ListenerService {
    /// Create a new listener service
    pub fn new(xds_state: Arc<XdsState>) -> Self {
        Self { xds_state }
    }

    /// Get the listener repository
    fn repository(&self) -> Result<ListenerRepository, Error> {
        self.xds_state
            .listener_repository
            .as_ref()
            .cloned()
            .ok_or_else(|| Error::internal("Listener repository not configured"))
    }

    /// Create a new listener
    pub async fn create_listener(
        &self,
        name: String,
        address: String,
        port: u16,
        protocol: String,
        config: ListenerConfig,
        team: Option<String>,
    ) -> Result<ListenerData, Error> {
        use opentelemetry::trace::{FutureExt, TraceContextExt};

        let mut span =
            create_operation_span("listener_service.create_listener", SpanKind::Internal);
        span.set_attribute(KeyValue::new("listener.name", name.clone()));
        span.set_attribute(KeyValue::new("listener.address", address.clone()));
        span.set_attribute(KeyValue::new("listener.port", port as i64));
        span.set_attribute(KeyValue::new("listener.protocol", protocol.clone()));

        let cx = opentelemetry::Context::current().with_span(span);

        async move {
            let repository = self.repository()?;

            let configuration = serde_json::to_value(&config).map_err(|err| {
                Error::internal(format!("Failed to serialize listener configuration: {}", err))
            })?;

            let request = CreateListenerRequest {
                name: name.clone(),
                address: address.clone(),
                port: Some(port as i64),
                protocol: Some(protocol.clone()),
                configuration,
                team,
                import_id: None,
            };

            let mut db_span = create_operation_span("db.listener.insert", SpanKind::Client);
            db_span.set_attribute(KeyValue::new("db.operation", "INSERT"));
            db_span.set_attribute(KeyValue::new("db.table", "listeners"));
            let created = repository.create(request).await?;
            drop(db_span);

            info!(
                listener_id = %created.id,
                listener_name = %created.name,
                "Listener created"
            );

            let mut xds_span = create_operation_span("xds.refresh_listeners", SpanKind::Internal);
            xds_span.set_attribute(KeyValue::new("listener.id", created.id.to_string()));
            self.refresh_xds().await?;
            drop(xds_span);

            Ok(created)
        }
        .with_context(cx)
        .await
    }

    /// List all listeners
    pub async fn list_listeners(
        &self,
        limit: Option<i32>,
        offset: Option<i32>,
    ) -> Result<Vec<ListenerData>, Error> {
        let repository = self.repository()?;
        repository.list(limit, offset).await
    }

    /// Get a listener by name
    pub async fn get_listener(&self, name: &str) -> Result<ListenerData, Error> {
        let repository = self.repository()?;
        repository.get_by_name(name).await
    }

    /// Update an existing listener
    pub async fn update_listener(
        &self,
        name: &str,
        address: String,
        port: u16,
        protocol: String,
        config: ListenerConfig,
    ) -> Result<ListenerData, Error> {
        use opentelemetry::trace::{FutureExt, TraceContextExt};

        let mut span =
            create_operation_span("listener_service.update_listener", SpanKind::Internal);
        span.set_attribute(KeyValue::new("listener.name", name.to_string()));
        span.set_attribute(KeyValue::new("listener.address", address.clone()));
        span.set_attribute(KeyValue::new("listener.port", port as i64));
        span.set_attribute(KeyValue::new("listener.protocol", protocol.clone()));

        let cx = opentelemetry::Context::current().with_span(span);

        async move {
            let repository = self.repository()?;
            let existing = repository.get_by_name(name).await?;

            let configuration = serde_json::to_value(&config).map_err(|err| {
                Error::internal(format!("Failed to serialize listener configuration: {}", err))
            })?;

            let update_request = UpdateListenerRequest {
                address: Some(address),
                port: Some(Some(port as i64)),
                protocol: Some(protocol),
                configuration: Some(configuration),
                team: None, // Don't modify team on update unless explicitly set
            };

            let mut db_span = create_operation_span("db.listener.update", SpanKind::Client);
            db_span.set_attribute(KeyValue::new("db.operation", "UPDATE"));
            db_span.set_attribute(KeyValue::new("db.table", "listeners"));
            let updated = repository.update(&existing.id, update_request).await?;
            drop(db_span);

            info!(
                listener_id = %updated.id,
                listener_name = %updated.name,
                "Listener updated"
            );

            let mut xds_span = create_operation_span("xds.refresh_listeners", SpanKind::Internal);
            xds_span.set_attribute(KeyValue::new("listener.id", updated.id.to_string()));
            self.refresh_xds().await?;
            drop(xds_span);

            Ok(updated)
        }
        .with_context(cx)
        .await
    }

    /// Delete a listener by name
    pub async fn delete_listener(&self, name: &str) -> Result<(), Error> {
        use opentelemetry::trace::{FutureExt, TraceContextExt};

        if is_default_gateway_listener(name) {
            return Err(Error::validation("The default gateway listener cannot be deleted"));
        }

        let mut span =
            create_operation_span("listener_service.delete_listener", SpanKind::Internal);
        span.set_attribute(KeyValue::new("listener.name", name.to_string()));

        let cx = opentelemetry::Context::current().with_span(span);

        async move {
            let repository = self.repository()?;
            let existing = repository.get_by_name(name).await?;

            let mut db_span = create_operation_span("db.listener.delete", SpanKind::Client);
            db_span.set_attribute(KeyValue::new("db.operation", "DELETE"));
            db_span.set_attribute(KeyValue::new("db.table", "listeners"));
            db_span.set_attribute(KeyValue::new("listener.id", existing.id.to_string()));
            repository.delete(&existing.id).await?;
            drop(db_span);

            info!(
                listener_id = %existing.id,
                listener_name = %existing.name,
                "Listener deleted"
            );

            let mut xds_span = create_operation_span("xds.refresh_listeners", SpanKind::Internal);
            xds_span.set_attribute(KeyValue::new("listener.id", existing.id.to_string()));
            self.refresh_xds().await?;
            drop(xds_span);

            Ok(())
        }
        .with_context(cx)
        .await
    }

    /// Parse listener configuration from stored JSON
    pub fn parse_config(&self, data: &ListenerData) -> Result<ListenerConfig, Error> {
        serde_json::from_str(&data.configuration).map_err(|err| {
            Error::internal(format!("Failed to parse stored listener configuration: {}", err))
        })
    }

    /// Refresh xDS caches after listener changes
    async fn refresh_xds(&self) -> Result<(), Error> {
        self.xds_state.refresh_listeners_from_repository().await.map_err(|err| {
            error!(error = %err, "Failed to refresh xDS caches after listener operation");
            err
        })
    }
}
