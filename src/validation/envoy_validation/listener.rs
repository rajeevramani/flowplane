use crate::errors::types::{FlowplaneError, Result};
use envoy_types::pb::envoy::config::listener::v3::{
    filter::ConfigType, Filter, FilterChain, Listener,
};

use super::{cluster::validate_address, helpers::encode_check};

pub fn validate_envoy_listener(listener: &Listener) -> Result<()> {
    encode_check(listener, "Invalid listener configuration")?;

    if listener.name.is_empty() {
        return Err(FlowplaneError::validation_field(
            "Listener name cannot be empty",
            "name",
        ));
    }

    match &listener.address {
        Some(address) => validate_address(address)?,
        None => {
            return Err(FlowplaneError::validation_field(
                "Listener address is required",
                "address",
            ))
        }
    }

    if listener.filter_chains.is_empty() {
        return Err(FlowplaneError::validation_field(
            "At least one filter chain is required",
            "filter_chains",
        ));
    }

    for (index, filter_chain) in listener.filter_chains.iter().enumerate() {
        validate_filter_chain(filter_chain).map_err(|e| {
            FlowplaneError::validation_field(
                format!("Filter chain {} validation failed: {}", index, e),
                "filter_chains",
            )
        })?;
    }

    Ok(())
}

fn validate_filter_chain(filter_chain: &FilterChain) -> Result<()> {
    if filter_chain.filters.is_empty() {
        return Err(FlowplaneError::validation("At least one filter is required"));
    }

    for (index, filter) in filter_chain.filters.iter().enumerate() {
        validate_filter(filter).map_err(|e| {
            FlowplaneError::validation(format!("Filter {} validation failed: {}", index, e))
        })?;
    }

    Ok(())
}

fn validate_filter(filter: &Filter) -> Result<()> {
    if filter.name.is_empty() {
        return Err(FlowplaneError::validation("Filter name cannot be empty"));
    }

    match &filter.config_type {
        Some(ConfigType::TypedConfig(_)) | Some(ConfigType::ConfigDiscovery()) => Ok(()),
        Some(ConfigType::HiddenEnvoyDeprecatedConfig(_)) | Some(ConfigType::None(_)) | None => Err(
            FlowplaneError::validation("Filter configuration is required"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use envoy_types::pb::envoy::config::core::v3::{
        address::Address as AddressType, socket_address::PortSpecifier, Address, SocketAddress,
    };

    #[test]
    fn envoy_listener_validation() {
        let listener = Listener {
            name: "test-listener".to_string(),
            address: Some(Address {
                address: Some(AddressType::SocketAddress(SocketAddress {
                    address: "0.0.0.0".to_string(),
                    port_specifier: Some(PortSpecifier::PortValue(8080)),
                    ..Default::default()
                })),
            }),
            filter_chains: vec![FilterChain {
                filters: vec![Filter {
                    name: "test-filter".to_string(),
                    config_type: Some(ConfigType::TypedConfig(prost_types::Any::default())),
                }],
                ..Default::default()
            }],
            ..Default::default()
        };

        assert!(validate_envoy_listener(&listener).is_ok());

        let invalid_listener = Listener {
            name: "".to_string(),
            ..Default::default()
        };

        assert!(validate_envoy_listener(&invalid_listener).is_err());
    }
}
