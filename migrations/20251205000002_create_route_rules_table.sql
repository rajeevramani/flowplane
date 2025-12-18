-- Migration: Create route_rules table
-- Purpose: Extract route rules from VirtualHost JSON into normalized table
--
-- This table stores route rule records extracted from virtual hosts.
-- Each route rule belongs to a virtual host and defines matching/action for traffic.
-- Route rules are synchronized when route configs are created/updated.
--
-- Route rules MUST have names for filter attachment to work.
-- If source config has unnamed rules, names are auto-generated from path patterns.

CREATE TABLE route_rules (
    id TEXT PRIMARY KEY,                          -- UUID
    virtual_host_id TEXT NOT NULL,                -- FK to virtual_hosts
    name TEXT NOT NULL,                           -- RouteRule name (required for attachment)
    path_pattern TEXT NOT NULL,                   -- Denormalized for display (prefix, exact, etc)
    match_type TEXT NOT NULL CHECK (              -- Type of path matching
        match_type IN ('prefix', 'exact', 'regex', 'path_template', 'connect_matcher')
    ),
    rule_order INTEGER NOT NULL,                  -- Position in routes array
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,

    FOREIGN KEY (virtual_host_id) REFERENCES virtual_hosts(id) ON DELETE CASCADE,
    UNIQUE(virtual_host_id, name)
);

-- Index for fast lookups by virtual host
CREATE INDEX idx_route_rules_virtual_host ON route_rules(virtual_host_id);
