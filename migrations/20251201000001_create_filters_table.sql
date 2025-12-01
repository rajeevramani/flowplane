-- Create filters table for resource-based filter management
CREATE TABLE filters (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    filter_type TEXT NOT NULL CHECK (filter_type IN ('header_mutation', 'jwt_auth', 'cors', 'rate_limit', 'ext_authz')),
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
