-- Create Api Definitions and Routes tables for Platform API abstraction
-- Migration: 20250115000001_create_api_definitions.sql

CREATE TABLE IF NOT EXISTS api_definitions (
    id TEXT PRIMARY KEY,
    team TEXT NOT NULL,
    domain TEXT NOT NULL,
    listener_isolation INTEGER NOT NULL DEFAULT 0,
    tls_config TEXT,
    metadata TEXT,
    bootstrap_uri TEXT,
    bootstrap_revision INTEGER NOT NULL DEFAULT 0,
    version INTEGER NOT NULL DEFAULT 1,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE(team, domain)
);

CREATE INDEX IF NOT EXISTS idx_api_definitions_team ON api_definitions(team);
CREATE INDEX IF NOT EXISTS idx_api_definitions_domain ON api_definitions(domain);
CREATE INDEX IF NOT EXISTS idx_api_definitions_updated_at ON api_definitions(updated_at);

CREATE TABLE IF NOT EXISTS api_routes (
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

    FOREIGN KEY (api_definition_id) REFERENCES api_definitions(id) ON DELETE CASCADE,
    UNIQUE(api_definition_id, match_type, match_value)
);

CREATE INDEX IF NOT EXISTS idx_api_routes_definition_id ON api_routes(api_definition_id);
CREATE INDEX IF NOT EXISTS idx_api_routes_match ON api_routes(match_type, match_value);
CREATE INDEX IF NOT EXISTS idx_api_routes_updated_at ON api_routes(updated_at);

-- Ensure cascading delete removes routes when parent definition is deleted
PRAGMA foreign_keys = ON;
