-- Reapers must operate with an explicit tenant/team predicate. Keep the expiry path
-- efficient for team-scoped deletion rather than encouraging global TTL scans.
CREATE INDEX idx_raw_observations_team_expires_at ON raw_observations(team_id, expires_at);
