-- Switch all team foreign keys from teams(name) to teams(id)
-- Migration: 20260207000002_switch_team_fk_to_team_id.sql
--
-- This migration converts all resource tables from using team names (TEXT)
-- to team IDs (UUIDs). This enables immutable team identity, supporting
-- future team renaming without cascading updates.
--
-- Strategy per table:
-- 1. Add team_id TEXT column (nullable temporarily)
-- 2. Populate via UPDATE ... FROM teams WHERE {table}.team = teams.name
-- 3. Make NOT NULL where required (10 tables NOT NULL, 3 NULLABLE)
-- 4. Drop old FK constraint
-- 5. Add new FK constraint to teams(id)
-- 6. Drop old indexes on team column
-- 7. Drop old team column
-- 8. Rename team_id to team
-- 9. Recreate indexes
--
-- Note: proxy_certificates already uses team_id -> teams(id), skip it

-- ============================================================================
-- 1. learning_sessions - NOT NULL team, CASCADE delete
-- ============================================================================

ALTER TABLE learning_sessions ADD COLUMN team_id TEXT;

UPDATE learning_sessions
SET team_id = teams.id
FROM teams
WHERE learning_sessions.team = teams.name;

ALTER TABLE learning_sessions ALTER COLUMN team_id SET NOT NULL;

ALTER TABLE learning_sessions DROP CONSTRAINT IF EXISTS fk_learning_sessions_team;

ALTER TABLE learning_sessions
    ADD CONSTRAINT fk_learning_sessions_team_id
    FOREIGN KEY (team_id) REFERENCES teams(id) ON DELETE CASCADE;

DROP INDEX IF EXISTS idx_learning_sessions_team;
DROP INDEX IF EXISTS idx_learning_sessions_team_status;
DROP INDEX IF EXISTS idx_learning_sessions_team_status_created;

ALTER TABLE learning_sessions DROP COLUMN team;

ALTER TABLE learning_sessions RENAME COLUMN team_id TO team;

CREATE INDEX idx_learning_sessions_team ON learning_sessions(team);
CREATE INDEX idx_learning_sessions_team_status ON learning_sessions(team, status);
CREATE INDEX idx_learning_sessions_team_status_created ON learning_sessions(team, status, created_at DESC);

-- ============================================================================
-- 2. inferred_schemas - NOT NULL team, no explicit FK to teams (relies on session CASCADE)
-- ============================================================================

ALTER TABLE inferred_schemas ADD COLUMN team_id TEXT;

UPDATE inferred_schemas
SET team_id = teams.id
FROM teams
WHERE inferred_schemas.team = teams.name;

ALTER TABLE inferred_schemas ALTER COLUMN team_id SET NOT NULL;

-- Drop FK constraint (named in 20251116000002_add_team_foreign_keys.sql)
ALTER TABLE inferred_schemas DROP CONSTRAINT IF EXISTS fk_inferred_schemas_team;
ALTER TABLE inferred_schemas DROP CONSTRAINT IF EXISTS inferred_schemas_team_fkey;

DROP INDEX IF EXISTS idx_inferred_schemas_team;
DROP INDEX IF EXISTS idx_inferred_schemas_team_method_path;

ALTER TABLE inferred_schemas DROP COLUMN team;

ALTER TABLE inferred_schemas RENAME COLUMN team_id TO team;

CREATE INDEX idx_inferred_schemas_team ON inferred_schemas(team);
CREATE INDEX idx_inferred_schemas_team_method_path ON inferred_schemas(team, http_method, path_pattern);

-- ============================================================================
-- 3. aggregated_api_schemas - NOT NULL team, add FK with CASCADE
-- ============================================================================

ALTER TABLE aggregated_api_schemas ADD COLUMN team_id TEXT;

UPDATE aggregated_api_schemas
SET team_id = teams.id
FROM teams
WHERE aggregated_api_schemas.team = teams.name;

ALTER TABLE aggregated_api_schemas ALTER COLUMN team_id SET NOT NULL;

-- Drop old FK if exists (this table previously had no FK to teams)
ALTER TABLE aggregated_api_schemas DROP CONSTRAINT IF EXISTS fk_aggregated_api_schemas_team;

-- Add new FK with CASCADE
ALTER TABLE aggregated_api_schemas
    ADD CONSTRAINT fk_aggregated_api_schemas_team_id
    FOREIGN KEY (team_id) REFERENCES teams(id) ON DELETE CASCADE;

DROP INDEX IF EXISTS idx_aggregated_schemas_team;
DROP INDEX IF EXISTS idx_aggregated_schemas_team_method_path;
DROP INDEX IF EXISTS idx_aggregated_schemas_team_method_path_version;

ALTER TABLE aggregated_api_schemas DROP COLUMN team;

ALTER TABLE aggregated_api_schemas RENAME COLUMN team_id TO team;

-- Need to drop and recreate UNIQUE constraint since it references the team column
ALTER TABLE aggregated_api_schemas DROP CONSTRAINT IF EXISTS aggregated_api_schemas_team_path_http_method_version_key;
ALTER TABLE aggregated_api_schemas ADD CONSTRAINT aggregated_api_schemas_team_path_http_method_version_key UNIQUE(team, path, http_method, version);

CREATE INDEX idx_aggregated_schemas_team ON aggregated_api_schemas(team);
CREATE INDEX idx_aggregated_schemas_team_method_path ON aggregated_api_schemas(team, http_method, path);
CREATE INDEX idx_aggregated_schemas_team_method_path_version ON aggregated_api_schemas(team, http_method, path, version DESC);

-- ============================================================================
-- 4. import_metadata - NOT NULL team, CASCADE delete
-- ============================================================================

ALTER TABLE import_metadata ADD COLUMN team_id TEXT;

UPDATE import_metadata
SET team_id = teams.id
FROM teams
WHERE import_metadata.team = teams.name;

ALTER TABLE import_metadata ALTER COLUMN team_id SET NOT NULL;

-- Inline FK auto-named by PostgreSQL as {table}_{column}_fkey
ALTER TABLE import_metadata DROP CONSTRAINT IF EXISTS import_metadata_team_fkey;
ALTER TABLE import_metadata DROP CONSTRAINT IF EXISTS fk_import_metadata_team;

ALTER TABLE import_metadata
    ADD CONSTRAINT fk_import_metadata_team_id
    FOREIGN KEY (team_id) REFERENCES teams(id) ON DELETE CASCADE;

DROP INDEX IF EXISTS idx_import_metadata_team;

ALTER TABLE import_metadata DROP COLUMN team;

ALTER TABLE import_metadata RENAME COLUMN team_id TO team;

-- Need to drop and recreate UNIQUE constraint
ALTER TABLE import_metadata DROP CONSTRAINT IF EXISTS import_metadata_team_spec_name_key;
ALTER TABLE import_metadata ADD CONSTRAINT import_metadata_team_spec_name_key UNIQUE(team, spec_name);

CREATE INDEX idx_import_metadata_team ON import_metadata(team);

-- ============================================================================
-- 5. filters - NOT NULL team, RESTRICT delete
-- ============================================================================

ALTER TABLE filters ADD COLUMN team_id TEXT;

UPDATE filters
SET team_id = teams.id
FROM teams
WHERE filters.team = teams.name;

ALTER TABLE filters ALTER COLUMN team_id SET NOT NULL;

ALTER TABLE filters DROP CONSTRAINT IF EXISTS filters_team_fkey;
ALTER TABLE filters DROP CONSTRAINT IF EXISTS fk_filters_team;

ALTER TABLE filters
    ADD CONSTRAINT fk_filters_team_id
    FOREIGN KEY (team_id) REFERENCES teams(id) ON DELETE RESTRICT;

DROP INDEX IF EXISTS idx_filters_team;
DROP INDEX IF EXISTS idx_filters_team_name;

ALTER TABLE filters DROP COLUMN team;

ALTER TABLE filters RENAME COLUMN team_id TO team;

-- Need to drop and recreate UNIQUE constraint
ALTER TABLE filters DROP CONSTRAINT IF EXISTS filters_team_name_key;
ALTER TABLE filters ADD CONSTRAINT filters_team_name_key UNIQUE(team, name);

CREATE INDEX idx_filters_team ON filters(team);
CREATE INDEX idx_filters_team_name ON filters(team, name);

-- ============================================================================
-- 6. secrets - NOT NULL team, RESTRICT delete
-- ============================================================================

ALTER TABLE secrets ADD COLUMN team_id TEXT;

UPDATE secrets
SET team_id = teams.id
FROM teams
WHERE secrets.team = teams.name;

ALTER TABLE secrets ALTER COLUMN team_id SET NOT NULL;

ALTER TABLE secrets DROP CONSTRAINT IF EXISTS secrets_team_fkey;
ALTER TABLE secrets DROP CONSTRAINT IF EXISTS fk_secrets_team;

ALTER TABLE secrets
    ADD CONSTRAINT fk_secrets_team_id
    FOREIGN KEY (team_id) REFERENCES teams(id) ON DELETE RESTRICT;

DROP INDEX IF EXISTS idx_secrets_team;
DROP INDEX IF EXISTS idx_secrets_team_name;

ALTER TABLE secrets DROP COLUMN team;

ALTER TABLE secrets RENAME COLUMN team_id TO team;

-- Need to drop and recreate UNIQUE constraint
ALTER TABLE secrets DROP CONSTRAINT IF EXISTS secrets_team_name_key;
ALTER TABLE secrets ADD CONSTRAINT secrets_team_name_key UNIQUE(team, name);

CREATE INDEX idx_secrets_team ON secrets(team);
CREATE INDEX idx_secrets_team_name ON secrets(team, name);

-- ============================================================================
-- 7. custom_wasm_filters - NOT NULL team, RESTRICT delete
-- ============================================================================

ALTER TABLE custom_wasm_filters ADD COLUMN team_id TEXT;

UPDATE custom_wasm_filters
SET team_id = teams.id
FROM teams
WHERE custom_wasm_filters.team = teams.name;

ALTER TABLE custom_wasm_filters ALTER COLUMN team_id SET NOT NULL;

ALTER TABLE custom_wasm_filters DROP CONSTRAINT IF EXISTS custom_wasm_filters_team_fkey;
ALTER TABLE custom_wasm_filters DROP CONSTRAINT IF EXISTS fk_custom_wasm_filters_team;

ALTER TABLE custom_wasm_filters
    ADD CONSTRAINT fk_custom_wasm_filters_team_id
    FOREIGN KEY (team_id) REFERENCES teams(id) ON DELETE RESTRICT;

DROP INDEX IF EXISTS idx_custom_wasm_filters_team;
DROP INDEX IF EXISTS idx_custom_wasm_filters_team_name;

ALTER TABLE custom_wasm_filters DROP COLUMN team;

ALTER TABLE custom_wasm_filters RENAME COLUMN team_id TO team;

-- Need to drop and recreate UNIQUE constraint
ALTER TABLE custom_wasm_filters DROP CONSTRAINT IF EXISTS custom_wasm_filters_team_name_key;
ALTER TABLE custom_wasm_filters ADD CONSTRAINT custom_wasm_filters_team_name_key UNIQUE(team, name);

CREATE INDEX idx_custom_wasm_filters_team ON custom_wasm_filters(team);
CREATE INDEX idx_custom_wasm_filters_team_name ON custom_wasm_filters(team, name);

-- ============================================================================
-- 8. mcp_tools - NOT NULL team, CASCADE delete
-- ============================================================================

ALTER TABLE mcp_tools ADD COLUMN team_id TEXT;

UPDATE mcp_tools
SET team_id = teams.id
FROM teams
WHERE mcp_tools.team = teams.name;

ALTER TABLE mcp_tools ALTER COLUMN team_id SET NOT NULL;

ALTER TABLE mcp_tools DROP CONSTRAINT IF EXISTS mcp_tools_team_fkey;
ALTER TABLE mcp_tools DROP CONSTRAINT IF EXISTS fk_mcp_tools_team;

ALTER TABLE mcp_tools
    ADD CONSTRAINT fk_mcp_tools_team_id
    FOREIGN KEY (team_id) REFERENCES teams(id) ON DELETE CASCADE;

DROP INDEX IF EXISTS idx_mcp_tools_team;
DROP INDEX IF EXISTS idx_mcp_tools_enabled;
DROP INDEX IF EXISTS idx_mcp_tools_category;
DROP INDEX IF EXISTS idx_mcp_tools_name;

ALTER TABLE mcp_tools DROP COLUMN team;

ALTER TABLE mcp_tools RENAME COLUMN team_id TO team;

-- Need to drop and recreate UNIQUE constraint
ALTER TABLE mcp_tools DROP CONSTRAINT IF EXISTS mcp_tools_team_name_key;
ALTER TABLE mcp_tools ADD CONSTRAINT mcp_tools_team_name_key UNIQUE(team, name);

CREATE INDEX idx_mcp_tools_team ON mcp_tools(team);
CREATE INDEX idx_mcp_tools_enabled ON mcp_tools(team, enabled);
CREATE INDEX idx_mcp_tools_category ON mcp_tools(team, category);
CREATE INDEX idx_mcp_tools_name ON mcp_tools(team, name);

-- ============================================================================
-- 9. dataplanes - NOT NULL team, CASCADE delete (matches original schema)
-- ============================================================================

ALTER TABLE dataplanes ADD COLUMN team_id TEXT;

UPDATE dataplanes
SET team_id = teams.id
FROM teams
WHERE dataplanes.team = teams.name;

ALTER TABLE dataplanes ALTER COLUMN team_id SET NOT NULL;

-- Inline FK auto-named by PostgreSQL
ALTER TABLE dataplanes DROP CONSTRAINT IF EXISTS dataplanes_team_fkey;
ALTER TABLE dataplanes DROP CONSTRAINT IF EXISTS fk_dataplanes_team;

ALTER TABLE dataplanes
    ADD CONSTRAINT fk_dataplanes_team_id
    FOREIGN KEY (team_id) REFERENCES teams(id) ON DELETE CASCADE;

DROP INDEX IF EXISTS idx_dataplanes_team;
DROP INDEX IF EXISTS idx_dataplanes_name;

ALTER TABLE dataplanes DROP COLUMN team;

ALTER TABLE dataplanes RENAME COLUMN team_id TO team;

-- Need to drop and recreate UNIQUE constraint
ALTER TABLE dataplanes DROP CONSTRAINT IF EXISTS dataplanes_team_name_key;
ALTER TABLE dataplanes ADD CONSTRAINT dataplanes_team_name_key UNIQUE(team, name);

CREATE INDEX idx_dataplanes_team ON dataplanes(team);
CREATE INDEX idx_dataplanes_name ON dataplanes(team, name);

-- ============================================================================
-- 10. user_team_memberships - NOT NULL team, CASCADE delete
-- ============================================================================

ALTER TABLE user_team_memberships ADD COLUMN team_id TEXT;

UPDATE user_team_memberships
SET team_id = teams.id
FROM teams
WHERE user_team_memberships.team = teams.name;

ALTER TABLE user_team_memberships ALTER COLUMN team_id SET NOT NULL;

ALTER TABLE user_team_memberships DROP CONSTRAINT IF EXISTS fk_user_team_memberships_team;

ALTER TABLE user_team_memberships
    ADD CONSTRAINT fk_user_team_memberships_team_id
    FOREIGN KEY (team_id) REFERENCES teams(id) ON DELETE CASCADE;

DROP INDEX IF EXISTS idx_user_team_memberships_team;
DROP INDEX IF EXISTS idx_user_team_memberships_user_team;

ALTER TABLE user_team_memberships DROP COLUMN team;

ALTER TABLE user_team_memberships RENAME COLUMN team_id TO team;

-- Need to recreate UNIQUE constraint (it will auto-update on rename in PostgreSQL)
CREATE UNIQUE INDEX idx_user_team_memberships_user_team ON user_team_memberships(user_id, team);
CREATE INDEX idx_user_team_memberships_team ON user_team_memberships(team);

-- ============================================================================
-- 11. clusters - NULLABLE team, RESTRICT delete
-- ============================================================================

ALTER TABLE clusters ADD COLUMN team_id TEXT;

UPDATE clusters
SET team_id = teams.id
FROM teams
WHERE clusters.team = teams.name;

-- team_id remains NULLABLE for clusters

ALTER TABLE clusters DROP CONSTRAINT IF EXISTS fk_clusters_team;

ALTER TABLE clusters
    ADD CONSTRAINT fk_clusters_team_id
    FOREIGN KEY (team_id) REFERENCES teams(id) ON DELETE RESTRICT;

DROP INDEX IF EXISTS idx_clusters_team;
DROP INDEX IF EXISTS idx_clusters_team_name;

ALTER TABLE clusters DROP COLUMN team;

ALTER TABLE clusters RENAME COLUMN team_id TO team;

CREATE INDEX idx_clusters_team ON clusters(team);
-- Recreate partial index for NULLABLE team
CREATE INDEX idx_clusters_team_name ON clusters(team, name) WHERE team IS NOT NULL;

-- ============================================================================
-- 12. route_configs (renamed from routes) - NULLABLE team, RESTRICT delete
-- ============================================================================

ALTER TABLE route_configs ADD COLUMN team_id TEXT;

UPDATE route_configs
SET team_id = teams.id
FROM teams
WHERE route_configs.team = teams.name;

-- team_id remains NULLABLE for route_configs

-- FK constraint still named fk_routes_team from before table rename
ALTER TABLE route_configs DROP CONSTRAINT IF EXISTS fk_routes_team;
ALTER TABLE route_configs DROP CONSTRAINT IF EXISTS fk_route_configs_team;

ALTER TABLE route_configs
    ADD CONSTRAINT fk_route_configs_team_id
    FOREIGN KEY (team_id) REFERENCES teams(id) ON DELETE RESTRICT;

-- Index was recreated as idx_route_configs_team after table rename
DROP INDEX IF EXISTS idx_route_configs_team;
DROP INDEX IF EXISTS idx_routes_team;
DROP INDEX IF EXISTS idx_routes_team_name;

ALTER TABLE route_configs DROP COLUMN team;

ALTER TABLE route_configs RENAME COLUMN team_id TO team;

CREATE INDEX idx_route_configs_team ON route_configs(team);
-- Recreate partial index for NULLABLE team
CREATE INDEX idx_route_configs_team_name ON route_configs(team, name) WHERE team IS NOT NULL;

-- ============================================================================
-- 13. listeners - NULLABLE team, RESTRICT delete
-- ============================================================================

ALTER TABLE listeners ADD COLUMN team_id TEXT;

UPDATE listeners
SET team_id = teams.id
FROM teams
WHERE listeners.team = teams.name;

-- team_id remains NULLABLE for listeners

ALTER TABLE listeners DROP CONSTRAINT IF EXISTS fk_listeners_team;

ALTER TABLE listeners
    ADD CONSTRAINT fk_listeners_team_id
    FOREIGN KEY (team_id) REFERENCES teams(id) ON DELETE RESTRICT;

DROP INDEX IF EXISTS idx_listeners_team;
DROP INDEX IF EXISTS idx_listeners_team_name;

ALTER TABLE listeners DROP COLUMN team;

ALTER TABLE listeners RENAME COLUMN team_id TO team;

CREATE INDEX idx_listeners_team ON listeners(team);
-- Recreate partial index for NULLABLE team
CREATE INDEX idx_listeners_team_name ON listeners(team, name) WHERE team IS NOT NULL;
