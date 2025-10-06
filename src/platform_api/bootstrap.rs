/// Compute a bootstrap artifact URI for the given API definition identifier.
///
/// The bootstrap configuration is now served dynamically via the
/// GET /api/v1/api-definitions/{id}/bootstrap endpoint rather than
/// being written to disk.
pub fn compute_bootstrap_uri(definition_id: &str) -> String {
    format!("/api/v1/api-definitions/{}/bootstrap", definition_id)
}
