/// Compute a bootstrap artifact URI for the given team.
///
/// The bootstrap configuration is now served dynamically via the
/// GET /api/v1/teams/{team}/bootstrap endpoint rather than
/// being written to disk. This is team-scoped, not API-definition-scoped.
pub fn compute_bootstrap_uri(team: &str) -> String {
    format!("/api/v1/teams/{}/bootstrap", team)
}
