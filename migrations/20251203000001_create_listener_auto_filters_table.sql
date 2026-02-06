-- Migration: Create listener_auto_filters table
-- Purpose: Track HTTP filters auto-added to listeners when filters are attached to routes
--
-- This table enables automatic listener filter chain management:
-- - When a filter is attached to a route, the required HTTP filter is auto-added to connected listeners
-- - When a filter is detached from a route, the HTTP filter is removed if no other routes need it
--
-- Fields:
-- - id: Unique record identifier
-- - listener_id: The listener that was modified
-- - http_filter_name: Envoy HTTP filter name (e.g., "envoy.filters.http.header_mutation")
-- - source_filter_id: The filter resource that triggered this auto-addition
-- - source_route_id: The route the filter was attached to
-- - created_at: When this auto-filter record was created

CREATE TABLE listener_auto_filters (
    id TEXT PRIMARY KEY,
    listener_id TEXT NOT NULL,
    http_filter_name TEXT NOT NULL,
    source_filter_id TEXT NOT NULL,
    source_route_id TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    FOREIGN KEY (listener_id) REFERENCES listeners(id) ON DELETE CASCADE,
    FOREIGN KEY (source_filter_id) REFERENCES filters(id) ON DELETE CASCADE,
    FOREIGN KEY (source_route_id) REFERENCES routes(id) ON DELETE CASCADE,

    -- Unique constraint: one tracking record per listener + filter + route combination
    UNIQUE(listener_id, http_filter_name, source_filter_id, source_route_id)
);

-- Index for fast lookups by listener and HTTP filter name (used during cleanup)
CREATE INDEX idx_listener_auto_filters_listener ON listener_auto_filters(listener_id);
CREATE INDEX idx_listener_auto_filters_http_name ON listener_auto_filters(listener_id, http_filter_name);

-- Index for finding records by source (used when detaching filter from route)
CREATE INDEX idx_listener_auto_filters_source ON listener_auto_filters(source_filter_id, source_route_id);
