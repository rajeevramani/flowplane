use crate::errors::types::{FlowplaneError, Result};

use super::helpers::is_valid_address_format;

/// Validate listener port and address constraints
pub fn validate_listener_address_port(address: &str, port: u32) -> Result<()> {
    if address.is_empty() {
        return Err(FlowplaneError::validation_field(
            "Listener address cannot be empty",
            "address",
        ));
    }

    if port == 0 || port > 65535 {
        return Err(FlowplaneError::validation_field(
            "Listener port must be between 1 and 65535",
            "port",
        ));
    }

    if port < 1024 {
        return Err(FlowplaneError::validation_field(
            "Ports below 1024 are reserved and cannot be used",
            "port",
        ));
    }

    if !is_valid_address_format(address) {
        return Err(FlowplaneError::validation_field(
            "Invalid address format",
            "address",
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listener_address_port_validation() {
        assert!(validate_listener_address_port("0.0.0.0", 8080).is_ok());
        assert!(validate_listener_address_port("127.0.0.1", 3000).is_ok());
        assert!(validate_listener_address_port("localhost", 8080).is_ok());

        assert!(validate_listener_address_port("", 8080).is_err());
        assert!(validate_listener_address_port("0.0.0.0", 0).is_err());
        assert!(validate_listener_address_port("0.0.0.0", 70000).is_err());
        assert!(validate_listener_address_port("0.0.0.0", 80).is_err());
    }
}
