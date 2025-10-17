//! Business rule helpers specific to the Platform API abstraction.

use crate::errors::{FlowplaneError, Result};
use crate::storage::{ApiDefinitionData, ApiRouteData};

/// Ensure the provided domain is available for the team, raising a validation error when a
/// conflicting definition already exists.
pub fn validate_domain_availability(
    existing: Option<&ApiDefinitionData>,
    requested_team: &str,
    requested_domain: &str,
) -> Result<()> {
    if let Some(record) = existing {
        let owner = record.team.as_str();
        if owner.eq_ignore_ascii_case(requested_team) {
            return Err(FlowplaneError::validation(format!(
                "Domain '{}' is already registered for team '{}'",
                requested_domain, owner
            )));
        }

        return Err(FlowplaneError::validation(format!(
            "Domain '{}' is already owned by team '{}'",
            requested_domain, owner
        )));
    }

    Ok(())
}

/// Validate that a new route matcher does not collide with an existing matcher.
/// Routes with the same path but different headers (e.g., different HTTP methods) are allowed.
pub fn validate_route_uniqueness(
    existing_routes: &[ApiRouteData],
    match_type: &str,
    match_value: &str,
    headers: Option<&serde_json::Value>,
) -> Result<()> {
    let collision = existing_routes.iter().any(|route| {
        // Routes match if they have the same match_type, match_value, AND headers
        let type_match = route.match_type.eq_ignore_ascii_case(match_type);
        let value_match = route.match_value.eq_ignore_ascii_case(match_value);

        // Compare headers: routes with different headers don't collide
        let headers_match = match (&route.headers, headers) {
            (None, None) => true, // Both have no headers - collision
            (Some(existing), Some(new)) => existing == new, // Same headers - collision
            _ => false,           // One has headers, one doesn't - no collision
        };

        type_match && value_match && headers_match
    });

    if collision {
        return Err(FlowplaneError::validation(format!(
            "Route matcher '{} {}' already exists",
            match_type, match_value
        )));
    }

    Ok(())
}

/// Prevent downgrading listener isolation once it has been enabled.
pub fn enforce_listener_isolation_transition(
    current_isolation: Option<bool>,
    requested_isolation: bool,
) -> Result<()> {
    if current_isolation.unwrap_or(false) && !requested_isolation {
        return Err(FlowplaneError::validation(
            "Listener isolation cannot be disabled once enabled for an API definition",
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_route(match_type: &str, match_value: &str) -> ApiRouteData {
        ApiRouteData {
            id: crate::domain::ApiRouteId::from_str_unchecked("route"),
            api_definition_id: crate::domain::ApiDefinitionId::from_str_unchecked("definition"),
            match_type: match_type.into(),
            match_value: match_value.into(),
            case_sensitive: true,
            headers: None,
            rewrite_prefix: None,
            rewrite_regex: None,
            rewrite_substitution: None,
            upstream_targets: serde_json::json!({ "targets": [] }),
            timeout_seconds: None,
            override_config: None,
            deployment_note: None,
            route_order: 0,
            generated_route_id: None,
            generated_cluster_id: None,
            filter_config: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn detects_domain_conflict_for_same_team() {
        let existing = ApiDefinitionData {
            id: crate::domain::ApiDefinitionId::from_str_unchecked("def"),
            team: "payments".into(),
            domain: "api.example.com".into(),
            listener_isolation: false,
            target_listeners: None,
            tls_config: None,
            metadata: None,
            bootstrap_uri: None,
            bootstrap_revision: 1,
            generated_listener_id: None,
            version: 1,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };

        let result = validate_domain_availability(Some(&existing), "payments", "api.example.com");
        assert!(result.is_err(), "collision should be detected for same team");
    }

    #[test]
    fn detects_route_collision() {
        let routes = vec![sample_route("prefix", "/v1/")];
        let result = validate_route_uniqueness(&routes, "prefix", "/v1/", None);
        assert!(result.is_err(), "matching route should be rejected");
    }

    #[test]
    fn prevents_isolation_downgrade() {
        let result = enforce_listener_isolation_transition(Some(true), false);
        assert!(result.is_err(), "downgrading isolation should be blocked");
    }

    #[test]
    fn allows_isolation_upgrade() {
        enforce_listener_isolation_transition(Some(false), true).expect("upgrade allowed");
        enforce_listener_isolation_transition(None, false).expect("default shared allowed");
    }
}
