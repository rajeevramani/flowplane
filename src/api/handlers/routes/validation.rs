//! Route validation and utility functions

use envoy_types::pb::envoy::extensions::path::r#match::uri_template::v3::UriTemplateMatchConfig;
use prost::Message;
use serde_json::Value;

use crate::{
    api::{error::ApiError, routes::ApiState},
    errors::Error,
    openapi::strip_gateway_tags,
    storage::{RouteData, RouteRepository},
    xds::route::RouteConfig as XdsRouteConfig,
};

use super::types::{
    PathMatchDefinition, RouteActionDefinition, RouteDefinition, RouteMatchDefinition,
    RouteResponse,
};

/// Extract route repository from API state
pub(super) fn require_route_repository(state: &ApiState) -> Result<RouteRepository, ApiError> {
    state
        .xds_state
        .route_repository
        .as_ref()
        .cloned()
        .ok_or_else(|| ApiError::service_unavailable("Route repository not configured"))
}

/// Convert database route data to API response
pub(super) fn route_response_from_data(data: RouteData) -> Result<RouteResponse, ApiError> {
    let mut value: Value = serde_json::from_str(&data.configuration).map_err(|err| {
        ApiError::from(Error::internal(format!(
            "Failed to parse stored route configuration: {}",
            err
        )))
    })?;

    strip_gateway_tags(&mut value);

    let xds_config: XdsRouteConfig = serde_json::from_value(value).map_err(|err| {
        ApiError::from(Error::internal(format!(
            "Failed to deserialize stored route configuration: {}",
            err
        )))
    })?;

    // Use team from database, or empty string if None (should not happen with explicit team requirement)
    let team = data.team.clone().unwrap_or_default();
    let config = RouteDefinition::from_xds_config(&xds_config, team.clone());

    Ok(RouteResponse {
        name: data.name,
        team,
        path_prefix: data.path_prefix,
        cluster_targets: data.cluster_name,
        config,
    })
}

/// Extract summary information from route definition for display
pub(super) fn summarize_route(definition: &RouteDefinition) -> (String, String) {
    let path_prefix = definition
        .virtual_hosts
        .iter()
        .flat_map(|vh| vh.routes.iter())
        .map(|route| match &route.r#match.path {
            PathMatchDefinition::Exact { value } | PathMatchDefinition::Prefix { value } => {
                value.clone()
            }
            PathMatchDefinition::Regex { value } => format!("regex:{}", value),
            PathMatchDefinition::Template { template } => format!("template:{}", template),
        })
        .next()
        .unwrap_or_else(|| "*".to_string());

    let cluster_summary = definition
        .virtual_hosts
        .iter()
        .flat_map(|vh| vh.routes.iter())
        .map(|route| match &route.action {
            RouteActionDefinition::Forward { cluster, .. } => cluster.clone(),
            RouteActionDefinition::Weighted { clusters, .. } => {
                clusters.first().map(|cluster| cluster.name.clone()).unwrap_or_default()
            }
            RouteActionDefinition::Redirect { .. } => "__redirect__".to_string(),
        })
        .next()
        .unwrap_or_else(|| "unknown".to_string());

    (path_prefix, cluster_summary)
}

/// Validate XDS route configuration by attempting Envoy conversion
pub(super) fn validate_route_config(config: XdsRouteConfig) -> Result<XdsRouteConfig, ApiError> {
    config.to_envoy_route_configuration().map_err(ApiError::from)?;
    Ok(config)
}

/// Validate route definition payload before persistence
pub(super) fn validate_route_payload(definition: &RouteDefinition) -> Result<(), ApiError> {
    use validator::Validate;

    definition.validate().map_err(|err| ApiError::from(Error::from(err)))?;

    for virtual_host in &definition.virtual_hosts {
        virtual_host.validate().map_err(|err| ApiError::from(Error::from(err)))?;

        if virtual_host.domains.iter().any(|domain| domain.trim().is_empty()) {
            return Err(validation_error("Virtual host domains must not be empty"));
        }

        for route in &virtual_host.routes {
            route.validate().map_err(|err| ApiError::from(Error::from(err)))?;
            validate_route_match(&route.r#match)?;
            validate_route_action(&route.action)?;

            match (&route.r#match.path, &route.action) {
                (
                    PathMatchDefinition::Template { .. },
                    RouteActionDefinition::Forward { prefix_rewrite: Some(_), .. },
                ) => {
                    return Err(validation_error(
                        "Template path matches do not support prefixRewrite",
                    ));
                }
                (PathMatchDefinition::Template { .. }, RouteActionDefinition::Forward { .. }) => {}
                (PathMatchDefinition::Template { .. }, _) => {
                    return Err(validation_error("Template path matches require a forward action"));
                }
                (_, RouteActionDefinition::Forward { template_rewrite: Some(_), .. }) => {
                    return Err(validation_error("templateRewrite requires a template path match"));
                }
                _ => {}
            }
        }
    }

    Ok(())
}

fn validate_route_match(r#match: &RouteMatchDefinition) -> Result<(), ApiError> {
    match &r#match.path {
        PathMatchDefinition::Exact { value } | PathMatchDefinition::Prefix { value } => {
            if value.trim().is_empty() {
                return Err(validation_error("Route match path value must not be empty"));
            }
        }
        PathMatchDefinition::Regex { value } => {
            if value.trim().is_empty() {
                return Err(validation_error("Route match path value must not be empty"));
            }
        }
        PathMatchDefinition::Template { template } => {
            if template.trim().is_empty() {
                return Err(validation_error("Route match template must not be empty"));
            }

            ensure_valid_uri_template(template)?;
        }
    }

    if r#match.headers.iter().any(|header| header.name.trim().is_empty()) {
        return Err(validation_error("Header match name must not be empty"));
    }

    if r#match.query_parameters.iter().any(|param| param.name.trim().is_empty()) {
        return Err(validation_error("Query parameter match name must not be empty"));
    }

    Ok(())
}

fn validate_route_action(action: &RouteActionDefinition) -> Result<(), ApiError> {
    match action {
        RouteActionDefinition::Forward { cluster, prefix_rewrite, template_rewrite, .. } => {
            if cluster.trim().is_empty() {
                return Err(validation_error("Forward action requires a cluster name"));
            }

            if let Some(prefix) = prefix_rewrite {
                if prefix.trim().is_empty() {
                    return Err(validation_error("prefixRewrite must not be an empty string"));
                }

                if !prefix.starts_with('/') {
                    return Err(validation_error("prefixRewrite must start with a slash"));
                }
            }

            if let Some(template) = template_rewrite {
                if template.trim().is_empty() {
                    return Err(validation_error("templateRewrite must not be an empty string"));
                }

                ensure_valid_uri_template(template)?;
            }
        }
        RouteActionDefinition::Weighted { clusters, .. } => {
            if clusters.is_empty() {
                return Err(validation_error("Weighted action must include at least one cluster"));
            }

            if clusters.iter().any(|cluster| cluster.name.trim().is_empty()) {
                return Err(validation_error("Weighted action cluster names must not be empty"));
            }

            if clusters.iter().any(|cluster| cluster.weight == 0) {
                return Err(validation_error(
                    "Weighted action cluster weights must be greater than zero",
                ));
            }
        }
        RouteActionDefinition::Redirect { host_redirect, path_redirect, .. } => {
            if host_redirect.as_ref().map(|s| s.trim().is_empty()).unwrap_or(false)
                || path_redirect.as_ref().map(|s| s.trim().is_empty()).unwrap_or(false)
            {
                return Err(validation_error("Redirect action values must not be empty strings"));
            }
        }
    }

    Ok(())
}

fn validation_error(message: impl Into<String>) -> ApiError {
    ApiError::from(Error::validation(message.into()))
}

fn ensure_valid_uri_template(template: &str) -> Result<(), ApiError> {
    let config = UriTemplateMatchConfig { path_template: template.to_string() };

    if config.encode_to_vec().is_empty() {
        Err(validation_error("Invalid URI template"))
    } else {
        Ok(())
    }
}
