-- Remove UNIQUE constraint on api_routes to allow multiple routes with same path but different headers
-- This is necessary for HTTP method extraction where routes differ by :method header matcher

-- SQLite doesn't support ALTER TABLE DROP CONSTRAINT, so we need to recreate the table

-- 1. Create new table without the UNIQUE constraint
CREATE TABLE api_routes_new (
    id TEXT PRIMARY KEY,
    api_definition_id TEXT NOT NULL,
    match_type TEXT NOT NULL,
    match_value TEXT NOT NULL,
    case_sensitive INTEGER NOT NULL DEFAULT 1,
    rewrite_prefix TEXT,
    rewrite_regex TEXT,
    rewrite_substitution TEXT,
    upstream_targets TEXT NOT NULL,
    timeout_seconds INTEGER,
    override_config TEXT,
    deployment_note TEXT,
    route_order INTEGER NOT NULL DEFAULT 0,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    generated_route_id TEXT REFERENCES routes(id) ON DELETE SET NULL,
    generated_cluster_id TEXT REFERENCES clusters(id) ON DELETE SET NULL,
    filter_config TEXT,
    headers TEXT,

    FOREIGN KEY (api_definition_id) REFERENCES api_definitions(id) ON DELETE CASCADE
);

-- 2. Copy data from old table
INSERT INTO api_routes_new SELECT * FROM api_routes;

-- 3. Drop old table
DROP TABLE api_routes;

-- 4. Rename new table
ALTER TABLE api_routes_new RENAME TO api_routes;

-- 5. Create index for performance on common queries
CREATE INDEX idx_api_routes_api_definition_id ON api_routes(api_definition_id);
CREATE INDEX idx_api_routes_match_type_value ON api_routes(match_type, match_value);
