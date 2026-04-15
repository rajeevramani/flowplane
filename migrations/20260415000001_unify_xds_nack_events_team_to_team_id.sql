-- Convert xds_nack_events.team from team NAME to team ID (FK to teams.id)
-- Migration: 20260415000001_unify_xds_nack_events_team_to_team_id.sql
-- Bug: fp-4g4
--
-- Background: persist_nack_event wrote the SPIFFE-extracted team NAME into
-- xds_nack_events.team, but ops_nack_history_handler queried the column with a
-- resolved team UUID. The two never matched, so `flowplane xds nacks` always
-- returned count:0 even when warming-failure rows existed in the DB.
--
-- Architect-approved fix (Option A): migrate the column to store team_id with
-- a real FK to teams(id), matching the precedent set by
-- 20260207000002_switch_team_fk_to_team_id.sql for dataplanes/clusters/etc.
-- Pattern: add team_id, backfill via name join, drop old column, rename, FK.
--
-- Backfill orphans: rows whose team name does NOT resolve to a teams row are
-- deleted. They are unreachable by definition (the bug meant no UI/CLI ever
-- saw them) and there is no value in carrying them forward. Project has no
-- customers, so this is a clean breaking change.

ALTER TABLE xds_nack_events ADD COLUMN team_id TEXT;

UPDATE xds_nack_events
SET team_id = teams.id
FROM teams
WHERE xds_nack_events.team = teams.name;

-- Drop any rows that did not resolve (stale team names from before this fix).
DELETE FROM xds_nack_events WHERE team_id IS NULL;

ALTER TABLE xds_nack_events ALTER COLUMN team_id SET NOT NULL;

-- Drop indexes that reference the old `team` column.
DROP INDEX IF EXISTS idx_xds_nack_events_team_created_at;
DROP INDEX IF EXISTS idx_xds_nack_events_team_dataplane;
DROP INDEX IF EXISTS idx_xds_nack_events_team_type_url;

ALTER TABLE xds_nack_events DROP COLUMN team;

ALTER TABLE xds_nack_events RENAME COLUMN team_id TO team;

ALTER TABLE xds_nack_events
    ADD CONSTRAINT fk_xds_nack_events_team_id
    FOREIGN KEY (team) REFERENCES teams(id) ON DELETE CASCADE;

CREATE INDEX idx_xds_nack_events_team_created_at ON xds_nack_events(team, created_at);
CREATE INDEX idx_xds_nack_events_team_dataplane ON xds_nack_events(team, dataplane_name);
CREATE INDEX idx_xds_nack_events_team_type_url ON xds_nack_events(team, type_url);
