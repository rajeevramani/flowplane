-- Create import_metadata table for tracking OpenAPI spec imports
-- Migration: 20251120000001_create_import_metadata_table.sql

CREATE TABLE IF NOT EXISTS import_metadata (
    id TEXT PRIMARY KEY,
    spec_name TEXT NOT NULL,
    spec_version TEXT,
    spec_checksum TEXT,  -- SHA256 hash of the OpenAPI spec for change detection
    team TEXT NOT NULL,
    source_content TEXT,  -- Optional: store the original OpenAPI spec
    imported_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,

    -- Foreign key to teams table
    FOREIGN KEY (team) REFERENCES teams(name) ON DELETE CASCADE,

    -- Ensure unique spec_name per team
    UNIQUE(team, spec_name)
);

-- Index for team-based queries
CREATE INDEX IF NOT EXISTS idx_import_metadata_team ON import_metadata(team);

-- Index for efficient timestamp queries
CREATE INDEX IF NOT EXISTS idx_import_metadata_updated_at ON import_metadata(updated_at);

-- Index for spec name lookups
CREATE INDEX IF NOT EXISTS idx_import_metadata_spec_name ON import_metadata(spec_name);
