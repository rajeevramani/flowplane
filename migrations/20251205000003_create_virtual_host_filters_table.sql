-- Migration: Create virtual_host_filters junction table
-- Purpose: Attach filters to virtual hosts for mid-level filter inheritance
--
-- Filters attached at the virtual host level apply to all route rules within
-- that virtual host. They override filters attached at the RouteConfig level
-- but are overridden by filters attached at the route rule level.
--
-- Inheritance hierarchy (most specific wins):
-- 1. route_filters (RouteConfig level) - applies to all
-- 2. virtual_host_filters - applies to specific vhost
-- 3. route_rule_filters - applies to specific rule

CREATE TABLE virtual_host_filters (
    virtual_host_id TEXT NOT NULL,
    filter_id TEXT NOT NULL,
    filter_order INTEGER NOT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,

    FOREIGN KEY (virtual_host_id) REFERENCES virtual_hosts(id) ON DELETE CASCADE,
    FOREIGN KEY (filter_id) REFERENCES filters(id) ON DELETE RESTRICT,

    PRIMARY KEY (virtual_host_id, filter_id),
    UNIQUE(virtual_host_id, filter_order)
);

-- Index for fast lookups by virtual host
CREATE INDEX idx_virtual_host_filters_vh ON virtual_host_filters(virtual_host_id);

-- Index for finding all attachments of a filter
CREATE INDEX idx_virtual_host_filters_filter ON virtual_host_filters(filter_id);
