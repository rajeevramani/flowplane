-- Migration: Create cluster_endpoints table
-- Purpose: Extract endpoints from cluster configuration JSON
--
-- This table normalizes endpoint data that was previously embedded in cluster
-- configuration JSON. Benefits:
-- - Direct queries for endpoint health status
-- - Indexed lookups for endpoint management
-- - Foundation for endpoint-level operations (health updates, weight changes)

CREATE TABLE cluster_endpoints (
    id TEXT PRIMARY KEY,
    cluster_id TEXT NOT NULL,
    address TEXT NOT NULL,
    port INTEGER NOT NULL,
    weight INTEGER NOT NULL DEFAULT 1,
    health_status TEXT NOT NULL DEFAULT 'unknown' CHECK (
        health_status IN ('healthy', 'unhealthy', 'degraded', 'unknown')
    ),
    priority INTEGER NOT NULL DEFAULT 0,      -- Locality priority
    metadata TEXT,                            -- JSON for endpoint metadata
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,

    FOREIGN KEY (cluster_id) REFERENCES clusters(id) ON DELETE CASCADE,
    UNIQUE(cluster_id, address, port)
);

-- Index for fast lookups by cluster
CREATE INDEX idx_cluster_endpoints_cluster ON cluster_endpoints(cluster_id);

-- Index for health status filtering
CREATE INDEX idx_cluster_endpoints_health ON cluster_endpoints(cluster_id, health_status);
