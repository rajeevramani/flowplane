-- Add optional name column to learning_sessions for human-friendly session identification
ALTER TABLE learning_sessions ADD COLUMN name TEXT;

-- Unique constraint on (team, name) where name is not null
-- Existing sessions get NULL name, new sessions get auto-generated names
CREATE UNIQUE INDEX idx_learning_sessions_team_name ON learning_sessions(team, name) WHERE name IS NOT NULL;
