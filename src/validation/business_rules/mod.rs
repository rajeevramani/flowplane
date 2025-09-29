//! Business-specific validation rules for the Platform API abstraction.

pub mod api_definition;

pub use api_definition::{
    enforce_listener_isolation_transition, validate_domain_availability, validate_route_uniqueness,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke_reexports() {
        let _ = validate_domain_availability(None, "team", "example.com");
        let _ = validate_route_uniqueness(&[], "prefix", "/");
        let _ = enforce_listener_isolation_transition(Some(false), true);
    }
}
