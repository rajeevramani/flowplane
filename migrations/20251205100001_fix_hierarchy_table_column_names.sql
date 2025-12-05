-- Migration: Fix hierarchy table column names for Envoy terminology alignment
--
-- This migration completes the Phase 8 rename by updating tables that were missed:
-- 1. virtual_hosts: route_id -> route_config_id
-- 2. listener_auto_filters: route_id -> route_config_id, source_route_rule_id -> source_route_id,
--    and update attachment_level CHECK values
--
-- Note: SQLite requires table recreation to rename columns with FK constraints

-- ============================================================================
-- Step 1: Fix virtual_hosts table
-- ============================================================================

-- Create new table with correct column name
CREATE TABLE virtual_hosts_new (
    id TEXT PRIMARY KEY,
    route_config_id TEXT NOT NULL,
    name TEXT NOT NULL,
    domains TEXT NOT NULL,
    rule_order INTEGER NOT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,

    FOREIGN KEY (route_config_id) REFERENCES route_configs(id) ON DELETE CASCADE,
    UNIQUE(route_config_id, name)
);

-- Copy data from old table
INSERT INTO virtual_hosts_new (id, route_config_id, name, domains, rule_order, created_at, updated_at)
SELECT id, route_id, name, domains, rule_order, created_at, updated_at FROM virtual_hosts;

-- Drop old table and rename new one
DROP TABLE virtual_hosts;
ALTER TABLE virtual_hosts_new RENAME TO virtual_hosts;

-- Recreate index
CREATE INDEX idx_virtual_hosts_route_config ON virtual_hosts(route_config_id);

-- ============================================================================
-- Step 2: Fix listener_auto_filters table
-- ============================================================================

-- Create new table with correct column names and CHECK constraint values
CREATE TABLE listener_auto_filters_new (
    id TEXT PRIMARY KEY,
    listener_id TEXT NOT NULL,
    http_filter_name TEXT NOT NULL,
    source_filter_id TEXT NOT NULL,

    -- Parent route config (always present for listener resolution)
    route_config_id TEXT NOT NULL,

    -- Attachment level discriminator (updated terminology)
    attachment_level TEXT NOT NULL CHECK (
        attachment_level IN ('route_config', 'virtual_host', 'route')
    ),

    -- Granular source IDs (nullable based on level)
    source_virtual_host_id TEXT,
    source_route_id TEXT,

    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,

    FOREIGN KEY (listener_id) REFERENCES listeners(id) ON DELETE CASCADE,
    FOREIGN KEY (source_filter_id) REFERENCES filters(id) ON DELETE CASCADE,
    FOREIGN KEY (route_config_id) REFERENCES route_configs(id) ON DELETE CASCADE,
    FOREIGN KEY (source_virtual_host_id) REFERENCES virtual_hosts(id) ON DELETE CASCADE,
    FOREIGN KEY (source_route_id) REFERENCES routes(id) ON DELETE CASCADE,

    -- Data integrity: correct source set for each level
    CHECK (
        (attachment_level = 'route_config'
            AND source_virtual_host_id IS NULL
            AND source_route_id IS NULL) OR
        (attachment_level = 'virtual_host'
            AND source_virtual_host_id IS NOT NULL
            AND source_route_id IS NULL) OR
        (attachment_level = 'route'
            AND source_route_id IS NOT NULL)
    ),

    UNIQUE(listener_id, http_filter_name, source_filter_id, route_config_id,
           attachment_level, source_virtual_host_id, source_route_id)
);

-- Copy data from old table with column and value mapping
-- Note: attachment_level values are mapped: 'route' -> 'route_config', 'route_rule' -> 'route'
INSERT INTO listener_auto_filters_new (
    id, listener_id, http_filter_name, source_filter_id, route_config_id,
    attachment_level, source_virtual_host_id, source_route_id, created_at
)
SELECT
    id, listener_id, http_filter_name, source_filter_id, route_id,
    CASE attachment_level
        WHEN 'route' THEN 'route_config'
        WHEN 'route_rule' THEN 'route'
        ELSE attachment_level
    END,
    source_virtual_host_id, source_route_rule_id, created_at
FROM listener_auto_filters;

-- Drop old table and rename new one
DROP TABLE listener_auto_filters;
ALTER TABLE listener_auto_filters_new RENAME TO listener_auto_filters;

-- Recreate indexes
CREATE INDEX idx_listener_auto_filters_listener ON listener_auto_filters(listener_id);
CREATE INDEX idx_listener_auto_filters_http_name ON listener_auto_filters(listener_id, http_filter_name);
CREATE INDEX idx_listener_auto_filters_source ON listener_auto_filters(source_filter_id, route_config_id);
CREATE INDEX idx_listener_auto_filters_vh ON listener_auto_filters(source_virtual_host_id);
CREATE INDEX idx_listener_auto_filters_route ON listener_auto_filters(source_route_id);
