-- Create teams table for multi-tenancy team management
-- Migration: 20251116000001_create_teams_table.sql
--
-- This table replaces free-form team strings with proper team entities to:
-- - Prevent typos when assigning teams to users/resources
-- - Enable centralized team lifecycle management
-- - Store team metadata (owner, settings, status)
-- - Support future features (quotas, hierarchies, billing)

CREATE TABLE IF NOT EXISTS teams (
    id TEXT PRIMARY KEY,                           -- UUID for team
    name TEXT NOT NULL UNIQUE,                     -- Unique team identifier (immutable)
    display_name TEXT NOT NULL,                    -- Human-readable team name
    description TEXT,                              -- Optional team description
    owner_user_id TEXT,                            -- Team owner (nullable)
    settings TEXT,                                 -- JSON settings (default filters, headers, etc.)
    status TEXT NOT NULL DEFAULT 'active',         -- active|inactive|archived
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (owner_user_id) REFERENCES users(id) ON DELETE SET NULL
);

-- Index on name for fast lookups (most common query pattern)
CREATE INDEX IF NOT EXISTS idx_teams_name ON teams(name);

-- Index on status for filtering active/inactive teams
CREATE INDEX IF NOT EXISTS idx_teams_status ON teams(status);

-- Index on owner for finding teams owned by a user
CREATE INDEX IF NOT EXISTS idx_teams_owner ON teams(owner_user_id);

-- Composite index for common query pattern (active teams)
CREATE INDEX IF NOT EXISTS idx_teams_status_name ON teams(status, name);
