-- Create routes table for storing Envoy route configurations
-- Migration: 20241201000002_create_routes_table.sql

CREATE TABLE IF NOT EXISTS routes (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    path_prefix TEXT NOT NULL,
    cluster_name TEXT NOT NULL,
    configuration TEXT NOT NULL,  -- JSON serialized route config
    version BIGINT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    -- Foreign key constraint to clusters table
    FOREIGN KEY (cluster_name) REFERENCES clusters(name) ON DELETE CASCADE,

    -- Ensure unique route names per version
    UNIQUE(name, version)
);

-- Index for version queries
CREATE INDEX IF NOT EXISTS idx_routes_version ON routes(version);

-- Index for cluster lookups
CREATE INDEX IF NOT EXISTS idx_routes_cluster_name ON routes(cluster_name);

-- Index for path prefix matching
CREATE INDEX IF NOT EXISTS idx_routes_path_prefix ON routes(path_prefix);

-- Index for efficient timestamp queries
CREATE INDEX IF NOT EXISTS idx_routes_updated_at ON routes(updated_at);