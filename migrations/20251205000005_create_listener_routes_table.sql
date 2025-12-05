-- Migration: Create listener_routes junction table
-- Purpose: Explicit listener -> route relationship (replaces JSON parsing)
--
-- This table normalizes the relationship between listeners and routes.
-- Previously, this relationship was embedded in the listener configuration JSON
-- and had to be parsed at runtime. This table enables:
-- - O(1) lookup for auto-filter resolution
-- - Direct queries for route-listener relationships
-- - Elimination of JSON parsing during xDS generation

CREATE TABLE listener_routes (
    listener_id TEXT NOT NULL,
    route_id TEXT NOT NULL,
    route_order INTEGER NOT NULL,             -- Order for RDS config names
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,

    FOREIGN KEY (listener_id) REFERENCES listeners(id) ON DELETE CASCADE,
    FOREIGN KEY (route_id) REFERENCES routes(id) ON DELETE CASCADE,

    PRIMARY KEY (listener_id, route_id),
    UNIQUE(listener_id, route_order)
);

-- Index for fast lookups by listener
CREATE INDEX idx_listener_routes_listener ON listener_routes(listener_id);

-- Index for finding which listeners use a specific route
CREATE INDEX idx_listener_routes_route ON listener_routes(route_id);
