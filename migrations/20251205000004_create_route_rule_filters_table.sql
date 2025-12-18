-- Migration: Create route_rule_filters junction table
-- Purpose: Attach filters to individual route rules for fine-grained control
--
-- Filters attached at the route rule level apply only to that specific rule.
-- They override filters attached at both RouteConfig and VirtualHost levels.
--
-- This enables scenarios like:
-- - Global rate limiting at RouteConfig level
-- - Stricter rate limits for specific endpoints at rule level
-- - JWT auth required only for admin routes

CREATE TABLE route_rule_filters (
    route_rule_id TEXT NOT NULL,
    filter_id TEXT NOT NULL,
    filter_order INTEGER NOT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,

    FOREIGN KEY (route_rule_id) REFERENCES route_rules(id) ON DELETE CASCADE,
    FOREIGN KEY (filter_id) REFERENCES filters(id) ON DELETE RESTRICT,

    PRIMARY KEY (route_rule_id, filter_id),
    UNIQUE(route_rule_id, filter_order)
);

-- Index for fast lookups by route rule
CREATE INDEX idx_route_rule_filters_rule ON route_rule_filters(route_rule_id);

-- Index for finding all attachments of a filter
CREATE INDEX idx_route_rule_filters_filter ON route_rule_filters(filter_id);
