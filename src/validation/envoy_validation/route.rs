use crate::errors::types::{MagayaError, Result};
use envoy_types::pb::envoy::config::route::v3::{
    route::Action as RouteActionEnum, route_match::PathSpecifier, Route, RouteAction, RouteConfiguration,
    RouteMatch, VirtualHost,
};

use super::{cluster::validate_address, helpers::encode_check};

pub fn validate_envoy_route_configuration(route_config: &RouteConfiguration) -> Result<()> {
    encode_check(route_config, "Invalid route configuration")?;

    if route_config.name.is_empty() {
        return Err(MagayaError::validation_field(
            "Route configuration name cannot be empty",
            "name",
        ));
    }

    if route_config.virtual_hosts.is_empty() {
        return Err(MagayaError::validation_field(
            "At least one virtual host is required",
            "virtual_hosts",
        ));
    }

    for (index, vhost) in route_config.virtual_hosts.iter().enumerate() {
        validate_virtual_host(vhost).map_err(|e| {
            MagayaError::validation_field(
                format!("Virtual host {} validation failed: {}", index, e),
                "virtual_hosts",
            )
        })?;
    }

    Ok(())
}

fn validate_virtual_host(vhost: &VirtualHost) -> Result<()> {
    if vhost.name.is_empty() {
        return Err(MagayaError::validation("Virtual host name cannot be empty"));
    }

    if vhost.domains.is_empty() {
        return Err(MagayaError::validation("At least one domain is required"));
    }

    for domain in &vhost.domains {
        if domain.is_empty() {
            return Err(MagayaError::validation("Domain cannot be empty"));
        }
    }

    if vhost.routes.is_empty() {
        return Err(MagayaError::validation("At least one route is required"));
    }

    for (index, route) in vhost.routes.iter().enumerate() {
        validate_route(route).map_err(|e| {
            MagayaError::validation(format!("Route {} validation failed: {}", index, e))
        })?;
    }

    Ok(())
}

fn validate_route(route: &Route) -> Result<()> {
    match &route.r#match {
        Some(route_match) => validate_route_match(route_match)?,
        None => return Err(MagayaError::validation("Route match is required")),
    }

    match &route.action {
        Some(RouteActionEnum::Route(route_action)) => validate_route_action(route_action),
        Some(_) => Ok(()),
        None => Err(MagayaError::validation("Route action is required")),
    }
}

fn validate_route_match(route_match: &RouteMatch) -> Result<()> {
    if route_match.path_specifier.is_none() {
        return Err(MagayaError::validation("Path specifier is required"));
    }

    Ok(())
}

fn validate_route_action(route_action: &RouteAction) -> Result<()> {
    match &route_action.cluster_specifier {
        Some(envoy_types::pb::envoy::config::route::v3::route_action::ClusterSpecifier::Cluster(name)) => {
            if name.is_empty() {
                Err(MagayaError::validation("Cluster name cannot be empty"))
            } else {
                Ok(())
            }
        }
        Some(
            envoy_types::pb::envoy::config::route::v3::route_action::ClusterSpecifier::ClusterHeader(header),
        ) => {
            if header.is_empty() {
                Err(MagayaError::validation("Cluster header cannot be empty"))
            } else {
                Ok(())
            }
        }
        Some(
            envoy_types::pb::envoy::config::route::v3::route_action::ClusterSpecifier::WeightedClusters(weighted),
        ) => {
            if weighted.clusters.is_empty() {
                return Err(MagayaError::validation("At least one weighted cluster is required"));
            }
            Ok(())
        }
        None => Err(MagayaError::validation("Cluster specifier is required")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envoy_route_configuration_validation() {
        let route_config = RouteConfiguration {
            name: "test-route".to_string(),
            virtual_hosts: vec![VirtualHost {
                name: "test-vhost".to_string(),
                domains: vec!["example.com".to_string()],
                routes: vec![Route {
                    r#match: Some(RouteMatch {
                        path_specifier: Some(PathSpecifier::Prefix("/".to_string())),
                        ..Default::default()
                    }),
                    action: Some(RouteActionEnum::Route(RouteAction {
                        cluster_specifier: Some(
                            envoy_types::pb::envoy::config::route::v3::route_action::ClusterSpecifier::Cluster(
                                "test-cluster".to_string(),
                            ),
                        ),
                        ..Default::default()
                    })),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };

        assert!(validate_envoy_route_configuration(&route_config).is_ok());

        let invalid_route = RouteConfiguration {
            name: "".to_string(),
            virtual_hosts: vec![],
            ..Default::default()
        };

        assert!(validate_envoy_route_configuration(&invalid_route).is_err());
    }
}
