-- Auto-aggregate support for learning sessions
ALTER TABLE learning_sessions ADD COLUMN auto_aggregate BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE learning_sessions ADD COLUMN snapshot_count BIGINT NOT NULL DEFAULT 0;

-- Link aggregated schemas to sessions and snapshots
ALTER TABLE aggregated_api_schemas ADD COLUMN session_id TEXT REFERENCES learning_sessions(id);
ALTER TABLE aggregated_api_schemas ADD COLUMN snapshot_number BIGINT;
CREATE INDEX idx_aggregated_schemas_session ON aggregated_api_schemas(session_id);
