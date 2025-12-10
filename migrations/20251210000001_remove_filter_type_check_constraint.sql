-- Remove filter_type CHECK constraint to enable dynamic filter types
-- The filter_type is now validated at the application level via FilterSchemaRegistry
-- SQLite requires table recreation to modify CHECK constraints

-- Drop old table (no data to preserve - pre-production)
DROP TABLE IF EXISTS filters;

-- Create table without filter_type CHECK constraint
CREATE TABLE filters (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    filter_type TEXT NOT NULL,  -- No CHECK constraint - validated by FilterSchemaRegistry
    description TEXT,
    configuration TEXT NOT NULL,
    version INTEGER NOT NULL DEFAULT 1,
    source TEXT NOT NULL DEFAULT 'native_api' CHECK (source IN ('native_api', 'ui', 'openapi_import')),
    team TEXT NOT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,

    FOREIGN KEY (team) REFERENCES teams(name) ON DELETE RESTRICT,
    UNIQUE(team, name)
);

CREATE INDEX idx_filters_team ON filters(team);
CREATE INDEX idx_filters_type ON filters(filter_type);
CREATE INDEX idx_filters_team_name ON filters(team, name);
