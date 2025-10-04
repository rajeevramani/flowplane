//! Listener business logic service
//!
//! This module contains the business logic for listener operations,
//! separated from HTTP concerns.

use std::sync::Arc;
use tracing::{error, info};

use crate::{
    errors::Error,
    openapi::defaults::is_default_gateway_listener,
    storage::{CreateListenerRequest, ListenerData, ListenerRepository, UpdateListenerRequest},
    xds::{listener::ListenerConfig, XdsState},
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
    ) -> Result<ListenerData, Error> {
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
        };

        let created = repository.create(request).await?;

        info!(
            listener_id = %created.id,
            listener_name = %created.name,
            "Listener created"
        );

        self.refresh_xds().await?;

        Ok(created)
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
        };

        let updated = repository.update(&existing.id, update_request).await?;

        info!(
            listener_id = %updated.id,
            listener_name = %updated.name,
            "Listener updated"
        );

        self.refresh_xds().await?;

        Ok(updated)
    }

    /// Delete a listener by name
    pub async fn delete_listener(&self, name: &str) -> Result<(), Error> {
        if is_default_gateway_listener(name) {
            return Err(Error::validation("The default gateway listener cannot be deleted"));
        }

        let repository = self.repository()?;
        let existing = repository.get_by_name(name).await?;

        repository.delete(&existing.id).await?;

        info!(
            listener_id = %existing.id,
            listener_name = %existing.name,
            "Listener deleted"
        );

        self.refresh_xds().await?;

        Ok(())
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
