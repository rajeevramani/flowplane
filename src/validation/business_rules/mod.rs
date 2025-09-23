//! Business-specific validation rules for Magaya control plane resources.

mod cluster;
mod helpers;
mod listener;
mod route;

pub use cluster::{
    validate_circuit_breaker_config, validate_cluster_naming_rules, validate_endpoint_weights,
    validate_health_check_config,
};
pub use listener::validate_listener_address_port;
pub use route::{
    validate_route_path_rewrite_compatibility, validate_virtual_host_domains,
};

#[cfg(test)]
mod tests {
    // Ensure public functions remain available at module root after refactor.
    use super::*;

    #[test]
    fn smoke_reexports() {
        let _ = validate_circuit_breaker_config(Some(1), Some(1), Some(1), Some(1));
        let _ = validate_endpoint_weights(&[Some(1)]);
        let _ = validate_health_check_config(5, 10, 2, 3, &None);
        let _ = validate_cluster_naming_rules("test", &[]);
        let _ = validate_route_path_rewrite_compatibility(
            "/", &crate::validation::PathMatchType::Prefix, &None, &None,
        );
        let _ = validate_virtual_host_domains(&["example.com".to_string()]);
        let _ = validate_listener_address_port("0.0.0.0", 1024);
    }
}
