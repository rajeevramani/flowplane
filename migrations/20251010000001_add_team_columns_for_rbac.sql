-- Add team columns to resource tables for RBAC team filtering
-- Migration: 20251010000001_add_team_columns_for_rbac.sql

-- Add team column to clusters table (nullable for backward compatibility)
ALTER TABLE clusters ADD COLUMN team TEXT;

-- Add team column to routes table (nullable for backward compatibility)
ALTER TABLE routes ADD COLUMN team TEXT;

-- Add team column to listeners table (nullable for backward compatibility)
ALTER TABLE listeners ADD COLUMN team TEXT;

-- Create indexes for team filtering performance
CREATE INDEX IF NOT EXISTS idx_clusters_team ON clusters(team);
CREATE INDEX IF NOT EXISTS idx_routes_team ON routes(team);
CREATE INDEX IF NOT EXISTS idx_listeners_team ON listeners(team);

-- Add composite indexes for common query patterns
CREATE INDEX IF NOT EXISTS idx_clusters_team_name ON clusters(team, name) WHERE team IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_routes_team_name ON routes(team, name) WHERE team IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_listeners_team_name ON listeners(team, name) WHERE team IS NOT NULL;
