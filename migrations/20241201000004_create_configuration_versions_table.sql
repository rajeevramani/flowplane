-- Create configuration_versions table for tracking global xDS version state
-- Migration: 20241201000004_create_configuration_versions_table.sql

CREATE TABLE IF NOT EXISTS configuration_versions (
    id INTEGER PRIMARY KEY,
    resource_type TEXT NOT NULL,  -- 'cluster', 'route', 'listener', 'endpoint'
    current_version INTEGER NOT NULL DEFAULT 1,
    last_updated DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,

    -- Ensure only one version entry per resource type
    UNIQUE(resource_type)
);

-- Insert initial version entries for each resource type
INSERT OR IGNORE INTO configuration_versions (resource_type, current_version) VALUES
    ('cluster', 1),
    ('route', 1),
    ('listener', 1),
    ('endpoint', 1);

-- Index for resource type lookups (though should be small table)
CREATE INDEX IF NOT EXISTS idx_config_versions_resource_type ON configuration_versions(resource_type);