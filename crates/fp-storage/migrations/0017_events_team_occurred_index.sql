-- Trace views page by team and recency; support that access path independently of the
-- consumer cursor index on (team_id, seq).
CREATE INDEX idx_events_team_occurred_at ON events(team_id, occurred_at DESC);
