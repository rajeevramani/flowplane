-- Migration: Create virtual_hosts table
-- Purpose: Extract virtual hosts from RouteConfig JSON into normalized table
--
-- This table stores virtual host records extracted from route configurations.
-- Each virtual host belongs to a route config and contains one or more route rules.
-- Virtual hosts are synchronized when route configs are created/updated.

CREATE TABLE virtual_hosts (
    id TEXT PRIMARY KEY,                          -- UUID
    route_id TEXT NOT NULL,                       -- FK to routes (RouteConfig)
    name TEXT NOT NULL,                           -- VirtualHost name
    domains TEXT NOT NULL,                        -- JSON array of domains
    rule_order INTEGER NOT NULL,                  -- Position in virtual_hosts array
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    FOREIGN KEY (route_id) REFERENCES routes(id) ON DELETE CASCADE,
    UNIQUE(route_id, name)
);

-- Index for fast lookups by route
CREATE INDEX idx_virtual_hosts_route ON virtual_hosts(route_id);
