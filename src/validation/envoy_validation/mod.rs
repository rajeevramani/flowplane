//! Envoy-types protocol validation helpers grouped by resource type.

mod cluster;
mod helpers;
mod listener;
mod route;

pub use cluster::validate_envoy_cluster;
pub use listener::validate_envoy_listener;
pub use route::validate_envoy_route_configuration;

use crate::errors::{FlowplaneError, Result};
use prost::Message;

/// Convenience function to validate any envoy-types `Message` via encoding.
pub fn validate_envoy_message<T: Message>(message: &T) -> Result<()> {
    if message.encode_to_vec().is_empty() {
        Err(FlowplaneError::validation(
            "Invalid configuration: failed envoy-types encoding",
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use envoy_types::pb::envoy::config::{
        cluster::v3::Cluster,
        route::v3::{route::Action as RouteActionEnum, route_match::PathSpecifier, Route, RouteAction, RouteConfiguration, RouteMatch, VirtualHost},
    };

    #[test]
    fn smoke_validate_message() {
        let cluster = Cluster {
            name: "test".to_string(),
            ..Default::default()
        };
        assert!(validate_envoy_message(&cluster).is_ok());
    }

    #[test]
    fn reexports_work() {
        let route = RouteConfiguration {
            name: "test".to_string(),
            virtual_hosts: vec![VirtualHost {
                name: "vh".to_string(),
                domains: vec!["*".to_string()],
                routes: vec![Route {
                    r#match: Some(RouteMatch {
                        path_specifier: Some(PathSpecifier::Prefix("/".to_string())),
                        ..Default::default()
                    }),
                    action: Some(RouteActionEnum::Route(RouteAction {
                        cluster_specifier: Some(
                            envoy_types::pb::envoy::config::route::v3::route_action::ClusterSpecifier::Cluster(
                                "cluster".to_string(),
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
        assert!(validate_envoy_route_configuration(&route).is_ok());
    }
}
