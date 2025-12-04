-- Add local_rate_limit to the filter_type CHECK constraint
-- SQLite requires table recreation to modify CHECK constraints

-- Create new table with updated constraint
CREATE TABLE filters_new (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    filter_type TEXT NOT NULL CHECK (filter_type IN ('header_mutation', 'jwt_auth', 'cors', 'local_rate_limit', 'rate_limit', 'ext_authz')),
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

-- Copy existing data
INSERT INTO filters_new SELECT * FROM filters;

-- Drop old table
DROP TABLE filters;

-- Rename new table
ALTER TABLE filters_new RENAME TO filters;

-- Recreate indexes
CREATE INDEX idx_filters_team ON filters(team);
CREATE INDEX idx_filters_type ON filters(filter_type);
CREATE INDEX idx_filters_team_name ON filters(team, name);
