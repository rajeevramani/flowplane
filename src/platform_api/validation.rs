use crate::errors::Result;
use crate::storage::ApiDefinitionRepository;
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
