-- Create cluster_references table for tracking cross-import cluster deduplication
-- Migration: 20251120000002_create_cluster_references_table.sql

CREATE TABLE IF NOT EXISTS cluster_references (
    cluster_id TEXT NOT NULL,
    import_id TEXT NOT NULL,
    route_count INTEGER NOT NULL DEFAULT 1,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,

    -- Foreign keys with CASCADE delete
    FOREIGN KEY (cluster_id) REFERENCES clusters(id) ON DELETE CASCADE,
    FOREIGN KEY (import_id) REFERENCES import_metadata(id) ON DELETE CASCADE,

    -- Primary key ensures one entry per cluster-import combination
    PRIMARY KEY (cluster_id, import_id)
);

-- Index for import-based lookups
CREATE INDEX IF NOT EXISTS idx_cluster_references_import ON cluster_references(import_id);

-- Index for cluster-based lookups
CREATE INDEX IF NOT EXISTS idx_cluster_references_cluster ON cluster_references(cluster_id);
