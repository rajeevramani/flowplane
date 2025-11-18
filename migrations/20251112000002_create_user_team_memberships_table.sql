-- Create user_team_memberships table for user-team relationships and scopes
-- Migration: 20251112000002_create_user_team_memberships_table.sql

CREATE TABLE IF NOT EXISTS user_team_memberships (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    team TEXT NOT NULL,
    scopes TEXT NOT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

-- Ensure a user can only belong to a team once
CREATE UNIQUE INDEX IF NOT EXISTS idx_user_team_memberships_user_team
    ON user_team_memberships(user_id, team);

-- Index for looking up all teams for a user
CREATE INDEX IF NOT EXISTS idx_user_team_memberships_user_id
    ON user_team_memberships(user_id);

-- Index for looking up all users in a team
CREATE INDEX IF NOT EXISTS idx_user_team_memberships_team
    ON user_team_memberships(team);
