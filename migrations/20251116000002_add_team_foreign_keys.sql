-- Add foreign key constraints from resource tables to teams table
-- Migration: 20251116000002_add_team_foreign_keys.sql
--
-- This migration adds FK constraints to enforce referential integrity between
-- team references and the teams table. Different delete policies are used:
-- - CASCADE: For ephemeral data that can be recreated (memberships, learning data)
-- - RESTRICT: For core resources to prevent accidental data loss
--
-- PostgreSQL: Use ALTER TABLE instead of SQLite table recreation pattern.
-- PostgreSQL supports ADD CONSTRAINT on existing tables natively.

-- ============================================================================
-- 1. user_team_memberships - CASCADE delete (membership is derived from team)
-- ============================================================================

ALTER TABLE user_team_memberships
    ADD CONSTRAINT fk_user_team_memberships_user_id
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE;

ALTER TABLE user_team_memberships
    ADD CONSTRAINT fk_user_team_memberships_team
    FOREIGN KEY (team) REFERENCES teams(name) ON DELETE CASCADE;

-- ============================================================================
-- 2. clusters - RESTRICT delete (core resource, conditional FK for NULL teams)
-- ============================================================================

-- Add source column for tracking origin API
ALTER TABLE clusters ADD COLUMN IF NOT EXISTS source TEXT NOT NULL DEFAULT 'native_api';
ALTER TABLE clusters ADD CONSTRAINT clusters_source_check
    CHECK (source IN ('native_api', 'openapi_import'));

-- Add FK to teams
ALTER TABLE clusters
    ADD CONSTRAINT fk_clusters_team
    FOREIGN KEY (team) REFERENCES teams(name) ON DELETE RESTRICT;

-- ============================================================================
-- 3. routes - RESTRICT delete (core resource, conditional FK for NULL teams)
-- ============================================================================

-- Add source column for tracking origin API
ALTER TABLE routes ADD COLUMN IF NOT EXISTS source TEXT NOT NULL DEFAULT 'native_api';
ALTER TABLE routes ADD CONSTRAINT routes_source_check
    CHECK (source IN ('native_api', 'openapi_import'));

-- Add FK to teams (FK to clusters already exists from original CREATE TABLE)
ALTER TABLE routes
    ADD CONSTRAINT fk_routes_team
    FOREIGN KEY (team) REFERENCES teams(name) ON DELETE RESTRICT;

-- ============================================================================
-- 4. listeners - RESTRICT delete (core resource, conditional FK for NULL teams)
-- ============================================================================

-- Add source column for tracking origin API
ALTER TABLE listeners ADD COLUMN IF NOT EXISTS source TEXT NOT NULL DEFAULT 'native_api';
ALTER TABLE listeners ADD CONSTRAINT listeners_source_check
    CHECK (source IN ('native_api', 'openapi_import'));

-- Add FK to teams
ALTER TABLE listeners
    ADD CONSTRAINT fk_listeners_team
    FOREIGN KEY (team) REFERENCES teams(name) ON DELETE RESTRICT;

-- ============================================================================
-- 5. learning_sessions - CASCADE delete (ephemeral data, can be recreated)
-- ============================================================================

ALTER TABLE learning_sessions
    ADD CONSTRAINT fk_learning_sessions_team
    FOREIGN KEY (team) REFERENCES teams(name) ON DELETE CASCADE;

-- ============================================================================
-- 6. inferred_schemas - CASCADE delete (ephemeral data, derived from learning)
-- ============================================================================

ALTER TABLE inferred_schemas
    ADD CONSTRAINT fk_inferred_schemas_team
    FOREIGN KEY (team) REFERENCES teams(name) ON DELETE CASCADE;

-- ============================================================================
-- 7. aggregated_api_schemas - CASCADE delete (ephemeral data, derived from schemas)
-- ============================================================================

ALTER TABLE aggregated_api_schemas
    ADD CONSTRAINT fk_aggregated_api_schemas_team
    FOREIGN KEY (team) REFERENCES teams(name) ON DELETE CASCADE;
