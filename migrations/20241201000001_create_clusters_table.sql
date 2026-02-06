-- Create clusters table for storing Envoy cluster configurations
-- Migration: 20241201000001_create_clusters_table.sql

CREATE TABLE IF NOT EXISTS clusters (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    service_name TEXT NOT NULL,
    configuration TEXT NOT NULL,  -- JSON serialized cluster config
    version BIGINT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    -- Indexing for performance
    UNIQUE(name, version)
);

-- Index for version queries (for xDS version management)
CREATE INDEX IF NOT EXISTS idx_clusters_version ON clusters(version);

-- Index for service name lookups
CREATE INDEX IF NOT EXISTS idx_clusters_service_name ON clusters(service_name);

-- Index for efficient timestamp queries
CREATE INDEX IF NOT EXISTS idx_clusters_updated_at ON clusters(updated_at);