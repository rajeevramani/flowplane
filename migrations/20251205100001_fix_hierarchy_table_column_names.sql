-- Migration: Fix hierarchy table column names for Envoy terminology alignment
--
-- This migration completes the Phase 8 rename by updating tables that were missed:
-- 1. virtual_hosts: route_id -> route_config_id
-- 2. listener_auto_filters: route_id -> route_config_id, source_route_rule_id -> source_route_id,
--    and update attachment_level CHECK values
--
-- PostgreSQL: Use ALTER TABLE RENAME COLUMN instead of table recreation

-- ============================================================================
-- Step 1: Fix virtual_hosts table
-- ============================================================================

ALTER TABLE virtual_hosts RENAME COLUMN route_id TO route_config_id;

-- Update index name to match new column name
DROP INDEX IF EXISTS idx_virtual_hosts_route;
CREATE INDEX IF NOT EXISTS idx_virtual_hosts_route_config ON virtual_hosts(route_config_id);

-- ============================================================================
-- Step 2: Fix listener_auto_filters table
-- ============================================================================

-- Rename columns
ALTER TABLE listener_auto_filters RENAME COLUMN route_id TO route_config_id;
ALTER TABLE listener_auto_filters RENAME COLUMN source_route_rule_id TO source_route_id;

-- Update attachment_level values to new terminology
UPDATE listener_auto_filters SET attachment_level = CASE
    WHEN attachment_level = 'route' THEN 'route_config'
    WHEN attachment_level = 'route_rule' THEN 'route'
    ELSE attachment_level
END;

-- Update the attachment_level CHECK constraint
ALTER TABLE listener_auto_filters DROP CONSTRAINT IF EXISTS listener_auto_filters_attachment_level_check;
ALTER TABLE listener_auto_filters ADD CONSTRAINT listener_auto_filters_attachment_level_check
    CHECK (attachment_level IN ('route_config', 'virtual_host', 'route'));

-- Update the data integrity CHECK constraint
ALTER TABLE listener_auto_filters DROP CONSTRAINT IF EXISTS listener_auto_filters_check;
ALTER TABLE listener_auto_filters ADD CONSTRAINT listener_auto_filters_check CHECK (
    (attachment_level = 'route_config'
        AND source_virtual_host_id IS NULL
        AND source_route_id IS NULL) OR
    (attachment_level = 'virtual_host'
        AND source_virtual_host_id IS NOT NULL
        AND source_route_id IS NULL) OR
    (attachment_level = 'route'
        AND source_route_id IS NOT NULL)
);

-- Update indexes for renamed columns
DROP INDEX IF EXISTS idx_listener_auto_filters_source;
CREATE INDEX idx_listener_auto_filters_source ON listener_auto_filters(source_filter_id, route_config_id);
DROP INDEX IF EXISTS idx_listener_auto_filters_rr;
CREATE INDEX idx_listener_auto_filters_route ON listener_auto_filters(source_route_id);
