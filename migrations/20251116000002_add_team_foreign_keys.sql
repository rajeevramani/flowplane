-- Add foreign key constraints from resource tables to teams table
-- Migration: 20251116000002_add_team_foreign_keys.sql
--
-- This migration adds FK constraints to enforce referential integrity between
-- team references and the teams table. Different delete policies are used:
-- - CASCADE: For ephemeral data that can be recreated (memberships, learning data)
-- - RESTRICT: For core resources to prevent accidental data loss
--
-- Note: SQLite doesn't support adding FK constraints to existing tables via ALTER TABLE.
-- We use the standard pattern: create new table with constraints, copy data, replace old table.

-- Enable foreign keys for this session
PRAGMA foreign_keys = ON;

-- ============================================================================
-- 1. user_team_memberships - CASCADE delete (membership is derived from team)
-- ============================================================================

CREATE TABLE user_team_memberships_new (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    team TEXT NOT NULL,
    scopes TEXT NOT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
    FOREIGN KEY (team) REFERENCES teams(name) ON DELETE CASCADE
);

-- Copy existing data
INSERT INTO user_team_memberships_new
SELECT id, user_id, team, scopes, created_at
FROM user_team_memberships;

-- Replace old table
DROP TABLE user_team_memberships;
ALTER TABLE user_team_memberships_new RENAME TO user_team_memberships;

-- Recreate indexes
CREATE UNIQUE INDEX IF NOT EXISTS idx_user_team_memberships_user_team
    ON user_team_memberships(user_id, team);
CREATE INDEX IF NOT EXISTS idx_user_team_memberships_user_id
    ON user_team_memberships(user_id);
CREATE INDEX IF NOT EXISTS idx_user_team_memberships_team
    ON user_team_memberships(team);

-- ============================================================================
-- 2. api_definitions - RESTRICT delete (core resource, prevent accidental loss)
-- ============================================================================

CREATE TABLE api_definitions_new (
    id TEXT PRIMARY KEY,
    team TEXT NOT NULL,
    domain TEXT NOT NULL,
    listener_isolation INTEGER NOT NULL DEFAULT 0,
    tls_config TEXT,
    metadata TEXT,
    bootstrap_uri TEXT,
    bootstrap_revision INTEGER NOT NULL DEFAULT 0,
    generated_listener_id TEXT,
    target_listeners TEXT,
    version INTEGER NOT NULL DEFAULT 1,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(team, domain),
    FOREIGN KEY (team) REFERENCES teams(name) ON DELETE RESTRICT
);

INSERT INTO api_definitions_new
SELECT id, team, domain, listener_isolation, tls_config, metadata,
       bootstrap_uri, bootstrap_revision, generated_listener_id, target_listeners,
       version, created_at, updated_at
FROM api_definitions;

DROP TABLE api_definitions;
ALTER TABLE api_definitions_new RENAME TO api_definitions;

CREATE INDEX IF NOT EXISTS idx_api_definitions_team ON api_definitions(team);
CREATE INDEX IF NOT EXISTS idx_api_definitions_domain ON api_definitions(domain);
CREATE INDEX IF NOT EXISTS idx_api_definitions_updated_at ON api_definitions(updated_at);
CREATE INDEX IF NOT EXISTS idx_api_definitions_listener ON api_definitions(generated_listener_id);
CREATE INDEX IF NOT EXISTS idx_api_definitions_target_listeners ON api_definitions(target_listeners) WHERE target_listeners IS NOT NULL;

-- ============================================================================
-- 3. clusters - RESTRICT delete (core resource, conditional FK for NULL teams)
-- ============================================================================

-- Note: clusters.team is nullable (for global resources)
-- FK constraint only applies when team IS NOT NULL

CREATE TABLE clusters_new (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    service_name TEXT NOT NULL,
    configuration TEXT NOT NULL,
    version INTEGER NOT NULL DEFAULT 1,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    source TEXT NOT NULL DEFAULT 'native_api' CHECK (source IN ('native_api', 'platform_api')),
    team TEXT,
    FOREIGN KEY (team) REFERENCES teams(name) ON DELETE RESTRICT,
    UNIQUE(name, version)
);

INSERT INTO clusters_new
SELECT id, name, service_name, configuration, version, created_at, updated_at, source, team
FROM clusters;

DROP TABLE clusters;
ALTER TABLE clusters_new RENAME TO clusters;

CREATE INDEX IF NOT EXISTS idx_clusters_team ON clusters(team);
CREATE INDEX IF NOT EXISTS idx_clusters_team_name ON clusters(team, name) WHERE team IS NOT NULL;

-- ============================================================================
-- 4. routes - RESTRICT delete (core resource, conditional FK for NULL teams)
-- ============================================================================

CREATE TABLE routes_new (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    path_prefix TEXT NOT NULL,
    cluster_name TEXT NOT NULL,
    configuration TEXT NOT NULL,
    version INTEGER NOT NULL DEFAULT 1,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    source TEXT NOT NULL DEFAULT 'native_api' CHECK (source IN ('native_api', 'platform_api')),
    team TEXT,
    FOREIGN KEY (cluster_name) REFERENCES clusters(name) ON DELETE CASCADE,
    FOREIGN KEY (team) REFERENCES teams(name) ON DELETE RESTRICT,
    UNIQUE(name, version)
);

INSERT INTO routes_new
SELECT id, name, path_prefix, cluster_name, configuration, version, created_at, updated_at, source, team
FROM routes;

DROP TABLE routes;
ALTER TABLE routes_new RENAME TO routes;

CREATE INDEX IF NOT EXISTS idx_routes_team ON routes(team);
CREATE INDEX IF NOT EXISTS idx_routes_team_name ON routes(team, name) WHERE team IS NOT NULL;

-- ============================================================================
-- 5. listeners - RESTRICT delete (core resource, conditional FK for NULL teams)
-- ============================================================================

CREATE TABLE listeners_new (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    address TEXT NOT NULL,
    port INTEGER,
    protocol TEXT NOT NULL DEFAULT 'HTTP',
    configuration TEXT NOT NULL,
    version INTEGER NOT NULL DEFAULT 1,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    source TEXT NOT NULL DEFAULT 'native_api' CHECK (source IN ('native_api', 'platform_api')),
    team TEXT,
    FOREIGN KEY (team) REFERENCES teams(name) ON DELETE RESTRICT,
    UNIQUE(name, version)
);

INSERT INTO listeners_new
SELECT id, name, address, port, protocol, configuration, version, created_at, updated_at, source, team
FROM listeners;

DROP TABLE listeners;
ALTER TABLE listeners_new RENAME TO listeners;

CREATE INDEX IF NOT EXISTS idx_listeners_team ON listeners(team);
CREATE INDEX IF NOT EXISTS idx_listeners_team_name ON listeners(team, name) WHERE team IS NOT NULL;

-- ============================================================================
-- 6. learning_sessions - CASCADE delete (ephemeral data, can be recreated)
-- ============================================================================

CREATE TABLE learning_sessions_new (
    id TEXT PRIMARY KEY,
    team TEXT NOT NULL,
    route_pattern TEXT NOT NULL,
    cluster_name TEXT,
    http_methods TEXT,
    status TEXT NOT NULL DEFAULT 'pending',
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    started_at DATETIME,
    ends_at DATETIME,
    completed_at DATETIME,
    target_sample_count INTEGER NOT NULL,
    current_sample_count INTEGER NOT NULL DEFAULT 0,
    triggered_by TEXT,
    deployment_version TEXT,
    configuration_snapshot TEXT,
    error_message TEXT,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (team) REFERENCES teams(name) ON DELETE CASCADE
);

INSERT INTO learning_sessions_new
SELECT id, team, route_pattern, cluster_name, http_methods, status, created_at,
       started_at, ends_at, completed_at, target_sample_count, current_sample_count,
       triggered_by, deployment_version, configuration_snapshot, error_message, updated_at
FROM learning_sessions;

DROP TABLE learning_sessions;
ALTER TABLE learning_sessions_new RENAME TO learning_sessions;

CREATE INDEX IF NOT EXISTS idx_learning_sessions_team ON learning_sessions(team);
CREATE INDEX IF NOT EXISTS idx_learning_sessions_status ON learning_sessions(status);

-- ============================================================================
-- 7. inferred_schemas - CASCADE delete (ephemeral data, derived from learning)
-- ============================================================================

CREATE TABLE inferred_schemas_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    team TEXT NOT NULL,
    session_id TEXT NOT NULL,
    http_method TEXT NOT NULL,
    path_pattern TEXT NOT NULL,
    request_schema TEXT,
    response_schema TEXT,
    response_status_code INTEGER,
    sample_count INTEGER NOT NULL DEFAULT 1,
    confidence REAL NOT NULL DEFAULT 1.0,
    first_seen_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    last_seen_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (session_id) REFERENCES learning_sessions(id) ON DELETE CASCADE,
    FOREIGN KEY (team) REFERENCES teams(name) ON DELETE CASCADE
);

INSERT INTO inferred_schemas_new
SELECT id, team, session_id, http_method, path_pattern, request_schema,
       response_schema, response_status_code, sample_count, confidence,
       first_seen_at, last_seen_at, created_at, updated_at
FROM inferred_schemas;

DROP TABLE inferred_schemas;
ALTER TABLE inferred_schemas_new RENAME TO inferred_schemas;

CREATE INDEX IF NOT EXISTS idx_inferred_schemas_team ON inferred_schemas(team);
CREATE INDEX IF NOT EXISTS idx_inferred_schemas_session ON inferred_schemas(session_id);

-- ============================================================================
-- 8. aggregated_api_schemas - CASCADE delete (ephemeral data, derived from schemas)
-- ============================================================================

CREATE TABLE aggregated_api_schemas_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    team TEXT NOT NULL,
    path TEXT NOT NULL,
    http_method TEXT NOT NULL,
    version INTEGER NOT NULL DEFAULT 1,
    previous_version_id INTEGER,
    request_schema TEXT,
    response_schemas TEXT,
    sample_count INTEGER NOT NULL DEFAULT 0,
    confidence_score REAL NOT NULL DEFAULT 0.0,
    breaking_changes TEXT,
    first_observed DATETIME NOT NULL,
    last_observed DATETIME NOT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (previous_version_id) REFERENCES aggregated_api_schemas(id),
    FOREIGN KEY (team) REFERENCES teams(name) ON DELETE CASCADE,
    UNIQUE(team, path, http_method, version)
);

INSERT INTO aggregated_api_schemas_new
SELECT id, team, path, http_method, version, previous_version_id, request_schema,
       response_schemas, sample_count, confidence_score, breaking_changes,
       first_observed, last_observed, created_at, updated_at
FROM aggregated_api_schemas;

DROP TABLE aggregated_api_schemas;
ALTER TABLE aggregated_api_schemas_new RENAME TO aggregated_api_schemas;

CREATE INDEX IF NOT EXISTS idx_aggregated_schemas_team ON aggregated_api_schemas(team);
CREATE INDEX IF NOT EXISTS idx_aggregated_schemas_path ON aggregated_api_schemas(path);

-- ============================================================================
-- Verification: Ensure foreign keys are enabled
-- ============================================================================

PRAGMA foreign_keys;
