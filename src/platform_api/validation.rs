use crate::errors::{Error as FlowplaneError, Result};
use crate::storage::{ApiDefinitionRepository, ListenerRepository};
use crate::validation::business_rules::{validate_domain_availability, validate_route_uniqueness};

/// Ensure the requested domain is available for the given team.
pub async fn ensure_domain_available(
    repo: &ApiDefinitionRepository,
    team: &str,
    domain: &str,
) -> Result<()> {
    let existing = repo.find_by_domain(domain).await?;
    validate_domain_availability(existing.as_ref(), team, domain)
}

/// Ensure the requested route matcher does not collide with existing routes.
pub async fn ensure_route_available(
    repo: &ApiDefinitionRepository,
    api_definition_id: &str,
    match_type: &str,
    match_value: &str,
) -> Result<()> {
    let existing_routes = repo.list_routes(api_definition_id).await?;
    validate_route_uniqueness(&existing_routes, match_type, match_value)
}

/// Ensure all target listeners exist in the database.
pub async fn ensure_target_listeners_exist(
    listener_repo: &ListenerRepository,
    target_listeners: &[String],
) -> Result<()> {
    for listener_name in target_listeners {
        // Try to fetch the listener by name - if it doesn't exist, this will return an error
        listener_repo.get_by_name(listener_name).await.map_err(|_| {
            FlowplaneError::validation(format!(
                "Target listener '{}' does not exist",
                listener_name
            ))
        })?;
    }
    Ok(())
}
