-- Migration: Recreate listener_auto_filters with full attachment level tracking
-- Purpose: Track HTTP filters auto-added to listeners with attachment level granularity
--
-- The enhanced table adds:
-- - attachment_level: Discriminator for route/virtual_host/route_rule
-- - source_virtual_host_id: Set when filter attached at VH level
-- - source_route_rule_id: Set when filter attached at RR level
--
-- Data integrity CHECK ensures correct source ID combinations for each level

-- Drop existing table (can be regenerated from current state)
DROP TABLE IF EXISTS listener_auto_filters;

CREATE TABLE listener_auto_filters (
    id TEXT PRIMARY KEY,
    listener_id TEXT NOT NULL,
    http_filter_name TEXT NOT NULL,
    source_filter_id TEXT NOT NULL,

    -- Parent route (always present for listener resolution)
    route_id TEXT NOT NULL,

    -- Attachment level discriminator
    attachment_level TEXT NOT NULL CHECK (
        attachment_level IN ('route', 'virtual_host', 'route_rule')
    ),

    -- Granular source IDs (nullable based on level)
    source_virtual_host_id TEXT,
    source_route_rule_id TEXT,

    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    FOREIGN KEY (listener_id) REFERENCES listeners(id) ON DELETE CASCADE,
    FOREIGN KEY (source_filter_id) REFERENCES filters(id) ON DELETE CASCADE,
    FOREIGN KEY (route_id) REFERENCES routes(id) ON DELETE CASCADE,
    FOREIGN KEY (source_virtual_host_id) REFERENCES virtual_hosts(id) ON DELETE CASCADE,
    FOREIGN KEY (source_route_rule_id) REFERENCES route_rules(id) ON DELETE CASCADE,

    -- Data integrity: correct source set for each level
    CHECK (
        (attachment_level = 'route'
            AND source_virtual_host_id IS NULL
            AND source_route_rule_id IS NULL) OR
        (attachment_level = 'virtual_host'
            AND source_virtual_host_id IS NOT NULL
            AND source_route_rule_id IS NULL) OR
        (attachment_level = 'route_rule'
            AND source_route_rule_id IS NOT NULL)
    ),

    UNIQUE(listener_id, http_filter_name, source_filter_id, route_id,
           attachment_level, source_virtual_host_id, source_route_rule_id)
);

CREATE INDEX idx_listener_auto_filters_listener ON listener_auto_filters(listener_id);
CREATE INDEX idx_listener_auto_filters_http_name ON listener_auto_filters(listener_id, http_filter_name);
CREATE INDEX idx_listener_auto_filters_source ON listener_auto_filters(source_filter_id, route_id);
CREATE INDEX idx_listener_auto_filters_vh ON listener_auto_filters(source_virtual_host_id);
CREATE INDEX idx_listener_auto_filters_rr ON listener_auto_filters(source_route_rule_id);
