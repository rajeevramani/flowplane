-- Migration: Normalize human scope seeds to fine-grained CRUD
-- Replaces all :write scopes with create/update/delete (and execute for tool resources).
--
-- Part A: Insert new CRUD scope rows for each :write scope, then delete :write rows.
-- Part B: Expand :write entries in user_team_memberships.scopes JSON arrays.
--
-- Idempotency:
--   Part A: ON CONFLICT (resource, action) DO NOTHING + DELETE WHERE action='write' (no-op second run)
--   Part B: WHERE scopes::text LIKE '%:write%' matches zero rows second run

-- ===========================================================================
-- Part A: Insert CRUD replacements for each :write scope
-- ===========================================================================

-- tokens:write → :create, :update, :delete (delete already exists)
INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled)
VALUES
    ('scope-tokens-create', 'tokens:create', 'tokens', 'create', 'Create tokens', 'Create personal access tokens', 'Tokens', TRUE, TRUE),
    ('scope-tokens-update', 'tokens:update', 'tokens', 'update', 'Update tokens', 'Modify personal access tokens', 'Tokens', TRUE, TRUE),
    ('scope-tokens-delete', 'tokens:delete', 'tokens', 'delete', 'Delete tokens', 'Revoke and delete personal access tokens', 'Tokens', TRUE, TRUE)
ON CONFLICT (resource, action) DO NOTHING;

-- clusters:write → :create, :update, :delete (delete already exists)
INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled)
VALUES
    ('scope-clusters-create', 'clusters:create', 'clusters', 'create', 'Create clusters', 'Create cluster configurations', 'Clusters', TRUE, TRUE),
    ('scope-clusters-update', 'clusters:update', 'clusters', 'update', 'Update clusters', 'Modify cluster configurations', 'Clusters', TRUE, TRUE),
    ('scope-clusters-delete', 'clusters:delete', 'clusters', 'delete', 'Delete clusters', 'Remove cluster configurations', 'Clusters', TRUE, TRUE)
ON CONFLICT (resource, action) DO NOTHING;

-- routes:write → :create, :update, :delete (delete already exists)
INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled)
VALUES
    ('scope-routes-create', 'routes:create', 'routes', 'create', 'Create routes', 'Create route configurations', 'Routes', TRUE, TRUE),
    ('scope-routes-update', 'routes:update', 'routes', 'update', 'Update routes', 'Modify route configurations', 'Routes', TRUE, TRUE),
    ('scope-routes-delete', 'routes:delete', 'routes', 'delete', 'Delete routes', 'Remove route configurations', 'Routes', TRUE, TRUE)
ON CONFLICT (resource, action) DO NOTHING;

-- listeners:write → :create, :update, :delete (delete already exists)
INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled)
VALUES
    ('scope-listeners-create', 'listeners:create', 'listeners', 'create', 'Create listeners', 'Create listener configurations', 'Listeners', TRUE, TRUE),
    ('scope-listeners-update', 'listeners:update', 'listeners', 'update', 'Update listeners', 'Modify listener configurations', 'Listeners', TRUE, TRUE),
    ('scope-listeners-delete', 'listeners:delete', 'listeners', 'delete', 'Delete listeners', 'Remove listener configurations', 'Listeners', TRUE, TRUE)
ON CONFLICT (resource, action) DO NOTHING;

-- openapi-import:write → :create, :update, :delete (delete already exists)
INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled)
VALUES
    ('scope-openapi-import-create', 'openapi-import:create', 'openapi-import', 'create', 'Create imports', 'Import OpenAPI specifications', 'OpenAPI Import', TRUE, TRUE),
    ('scope-openapi-import-update', 'openapi-import:update', 'openapi-import', 'update', 'Update imports', 'Modify OpenAPI imports', 'OpenAPI Import', TRUE, TRUE),
    ('scope-openapi-import-delete', 'openapi-import:delete', 'openapi-import', 'delete', 'Delete imports', 'Remove OpenAPI imports', 'OpenAPI Import', TRUE, TRUE)
ON CONFLICT (resource, action) DO NOTHING;

-- filters:write → :create, :update, :delete (delete already exists)
INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled)
VALUES
    ('scope-filters-create', 'filters:create', 'filters', 'create', 'Create filters', 'Create filter configurations', 'Filters', TRUE, TRUE),
    ('scope-filters-update', 'filters:update', 'filters', 'update', 'Update filters', 'Modify filter configurations', 'Filters', TRUE, TRUE),
    ('scope-filters-delete', 'filters:delete', 'filters', 'delete', 'Delete filters', 'Remove filter configurations', 'Filters', TRUE, TRUE)
ON CONFLICT (resource, action) DO NOTHING;

-- secrets:write → :create, :update, :delete (delete already exists)
INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled)
VALUES
    ('scope-secrets-create', 'secrets:create', 'secrets', 'create', 'Create secrets', 'Create secret values', 'Secrets', TRUE, TRUE),
    ('scope-secrets-update', 'secrets:update', 'secrets', 'update', 'Update secrets', 'Modify secret values', 'Secrets', TRUE, TRUE),
    ('scope-secrets-delete', 'secrets:delete', 'secrets', 'delete', 'Delete secrets', 'Remove secrets from the system', 'Secrets', TRUE, TRUE)
ON CONFLICT (resource, action) DO NOTHING;

-- custom-wasm-filters:write → :create, :update, :delete (delete already exists)
INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled)
VALUES
    ('scope-custom-wasm-filters-create', 'custom-wasm-filters:create', 'custom-wasm-filters', 'create', 'Create custom filters', 'Upload custom WASM filters', 'Custom Filters', TRUE, TRUE),
    ('scope-custom-wasm-filters-update', 'custom-wasm-filters:update', 'custom-wasm-filters', 'update', 'Update custom filters', 'Modify custom WASM filter configurations', 'Custom Filters', TRUE, TRUE),
    ('scope-custom-wasm-filters-delete', 'custom-wasm-filters:delete', 'custom-wasm-filters', 'delete', 'Delete custom filters', 'Remove custom WASM filters', 'Custom Filters', TRUE, TRUE)
ON CONFLICT (resource, action) DO NOTHING;

-- learning-sessions:write → :create, :execute only (no :update; control semantics not CRUD update)
-- learning-sessions:delete already exists from 20251228000001_add_missing_scopes.sql
INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled)
VALUES
    ('scope-learning-sessions-create', 'learning-sessions:create', 'learning-sessions', 'create', 'Create learning sessions', 'Start new API learning sessions', 'Learning', TRUE, TRUE),
    ('scope-learning-sessions-execute', 'learning-sessions:execute', 'learning-sessions', 'execute', 'Control learning sessions', 'Start, stop, and configure running learning sessions', 'Learning', TRUE, TRUE)
ON CONFLICT (resource, action) DO NOTHING;

-- aggregated-schemas:write → :create, :update, :delete
INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled)
VALUES
    ('scope-aggregated-schemas-create', 'aggregated-schemas:create', 'aggregated-schemas', 'create', 'Create API schemas', 'Create learned API schemas', 'Schemas', TRUE, TRUE),
    ('scope-aggregated-schemas-update', 'aggregated-schemas:update', 'aggregated-schemas', 'update', 'Update API schemas', 'Modify learned API schemas', 'Schemas', TRUE, TRUE),
    ('scope-aggregated-schemas-delete', 'aggregated-schemas:delete', 'aggregated-schemas', 'delete', 'Delete API schemas', 'Remove learned API schemas', 'Schemas', TRUE, TRUE)
ON CONFLICT (resource, action) DO NOTHING;

-- admin:orgs:write → :create, :update (delete already exists)
INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled)
VALUES
    ('scope-admin-orgs-create', 'admin:orgs:create', 'admin-orgs', 'create', 'Create organizations', 'Create new organizations on the platform', 'Admin', FALSE, TRUE),
    ('scope-admin-orgs-update', 'admin:orgs:update', 'admin-orgs', 'update', 'Update organizations', 'Modify organization details', 'Admin', FALSE, TRUE),
    ('scope-admin-orgs-delete', 'admin:orgs:delete', 'admin-orgs', 'delete', 'Delete organizations', 'Remove organizations from the platform', 'Admin', FALSE, TRUE)
ON CONFLICT (resource, action) DO NOTHING;

-- admin:users:write → :create, :update (delete already exists)
INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled)
VALUES
    ('scope-admin-users-create', 'admin:users:create', 'admin-users', 'create', 'Create users', 'Create new user accounts', 'Admin', FALSE, TRUE),
    ('scope-admin-users-update', 'admin:users:update', 'admin-users', 'update', 'Update users', 'Modify user accounts', 'Admin', FALSE, TRUE),
    ('scope-admin-users-delete', 'admin:users:delete', 'admin-users', 'delete', 'Delete users', 'Remove user accounts from the platform', 'Admin', FALSE, TRUE)
ON CONFLICT (resource, action) DO NOTHING;

-- admin:teams:write → :create, :update (delete already exists)
INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled)
VALUES
    ('scope-admin-teams-create', 'admin:teams:create', 'admin-teams', 'create', 'Create teams (admin)', 'Create teams via admin endpoints', 'Admin', FALSE, TRUE),
    ('scope-admin-teams-update', 'admin:teams:update', 'admin-teams', 'update', 'Update teams (admin)', 'Modify teams via admin endpoints', 'Admin', FALSE, TRUE),
    ('scope-admin-teams-delete', 'admin:teams:delete', 'admin-teams', 'delete', 'Delete teams (admin)', 'Remove teams via admin endpoints', 'Admin', FALSE, TRUE)
ON CONFLICT (resource, action) DO NOTHING;

-- admin:apps:write → :create, :update, :delete
INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled)
VALUES
    ('scope-admin-apps-create', 'admin:apps:create', 'admin-apps', 'create', 'Create instance apps', 'Create instance application configurations', 'Admin', FALSE, TRUE),
    ('scope-admin-apps-update', 'admin:apps:update', 'admin-apps', 'update', 'Update instance apps', 'Update instance application configurations', 'Admin', FALSE, TRUE),
    ('scope-admin-apps-delete', 'admin:apps:delete', 'admin-apps', 'delete', 'Delete instance apps', 'Remove instance application configurations', 'Admin', FALSE, TRUE)
ON CONFLICT (resource, action) DO NOTHING;

-- admin:filter-schemas:write → :create, :update, :delete
INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled)
VALUES
    ('scope-admin-filter-schemas-create', 'admin:filter-schemas:create', 'admin-filter-schemas', 'create', 'Create filter schemas', 'Create filter schema definitions', 'Admin', FALSE, TRUE),
    ('scope-admin-filter-schemas-update', 'admin:filter-schemas:update', 'admin-filter-schemas', 'update', 'Update filter schemas', 'Reload filter schema definitions from disk', 'Admin', FALSE, TRUE),
    ('scope-admin-filter-schemas-delete', 'admin:filter-schemas:delete', 'admin-filter-schemas', 'delete', 'Delete filter schemas', 'Remove filter schema definitions', 'Admin', FALSE, TRUE)
ON CONFLICT (resource, action) DO NOTHING;

-- cp:write → :create, :update, :delete, :execute (tool semantics)
INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled)
VALUES
    ('scope-cp-create', 'cp:create', 'cp', 'create', 'Create Control Plane resources', 'Create control plane resources via MCP tools', 'MCP', TRUE, TRUE),
    ('scope-cp-update', 'cp:update', 'cp', 'update', 'Update Control Plane resources', 'Update control plane resources via MCP tools', 'MCP', TRUE, TRUE),
    ('scope-cp-delete', 'cp:delete', 'cp', 'delete', 'Delete Control Plane resources', 'Delete control plane resources via MCP tools', 'MCP', TRUE, TRUE),
    ('scope-cp-execute', 'cp:execute', 'cp', 'execute', 'Execute Control Plane tools', 'Execute control plane MCP tools', 'MCP', TRUE, TRUE)
ON CONFLICT (resource, action) DO NOTHING;

-- mcp:write → :create, :update, :delete, :execute (tool semantics)
-- mcp:execute was previously deleted in 20260228000001_remove_old_mcp_scopes.sql; re-adding here
INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled)
VALUES
    ('scope-mcp-create', 'mcp:create', 'mcp', 'create', 'Create MCP tool configurations', 'Create MCP tool configurations', 'MCP', TRUE, TRUE),
    ('scope-mcp-update', 'mcp:update', 'mcp', 'update', 'Update MCP tool configurations', 'Modify MCP tool configurations', 'MCP', TRUE, TRUE),
    ('scope-mcp-delete', 'mcp:delete', 'mcp', 'delete', 'Delete MCP tool configurations', 'Remove MCP tool configurations', 'MCP', TRUE, TRUE),
    ('scope-mcp-execute', 'mcp:execute', 'mcp', 'execute', 'Execute MCP Tools', 'Execute MCP gateway tools (call upstream APIs)', 'MCP', TRUE, TRUE)
ON CONFLICT (resource, action) DO NOTHING;

-- dataplanes:write → :create, :update, :delete
INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled)
VALUES
    ('scope-dataplanes-create', 'dataplanes:create', 'dataplanes', 'create', 'Create Dataplanes', 'Create new dataplane configurations', 'Configuration', TRUE, TRUE),
    ('scope-dataplanes-update', 'dataplanes:update', 'dataplanes', 'update', 'Update Dataplanes', 'Modify dataplane configurations', 'Configuration', TRUE, TRUE),
    ('scope-dataplanes-delete', 'dataplanes:delete', 'dataplanes', 'delete', 'Delete Dataplanes', 'Remove dataplane configurations', 'Configuration', TRUE, TRUE)
ON CONFLICT (resource, action) DO NOTHING;

-- agents:write → :create, :update, :delete
INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled)
VALUES
    ('scope-agents-create', 'agents:create', 'agents', 'create', 'Create agents', 'Create agents in your team', 'Resources', TRUE, TRUE),
    ('scope-agents-update', 'agents:update', 'agents', 'update', 'Update agents', 'Modify agents in your team', 'Resources', TRUE, TRUE),
    ('scope-agents-delete', 'agents:delete', 'agents', 'delete', 'Delete agents', 'Remove agents from your team', 'Resources', TRUE, TRUE)
ON CONFLICT (resource, action) DO NOTHING;

-- Remove all :write scopes (replaced by fine-grained CRUD above)
DELETE FROM scopes WHERE action = 'write';

-- ===========================================================================
-- Part B: Expand :write entries in user_team_memberships.scopes JSON arrays
-- ===========================================================================
-- Rules:
--   Standard resources:      :write → :create + :update + :delete
--   learning-sessions:write → :create + :execute  (no :update, no :delete from write)
--   cp:write                → :create + :update + :delete + :execute
--   mcp:write               → :create + :update + :delete + :execute

UPDATE user_team_memberships
SET scopes = (
    SELECT jsonb_agg(s)::text
    FROM (
        SELECT DISTINCT elem AS s
        FROM (
            -- Keep non-write scopes unchanged
            SELECT elem FROM jsonb_array_elements_text(scopes::jsonb) AS elem
            WHERE elem NOT LIKE '%:write'
            UNION ALL
            -- Expand :write → :create (all write resources)
            SELECT replace(elem, ':write', ':create')
            FROM jsonb_array_elements_text(scopes::jsonb) AS elem
            WHERE elem LIKE '%:write'
            UNION ALL
            -- Expand :write → :update (all except learning-sessions)
            SELECT replace(elem, ':write', ':update')
            FROM jsonb_array_elements_text(scopes::jsonb) AS elem
            WHERE elem LIKE '%:write' AND elem != 'learning-sessions:write'
            UNION ALL
            -- Expand :write → :delete (all except learning-sessions)
            SELECT replace(elem, ':write', ':delete')
            FROM jsonb_array_elements_text(scopes::jsonb) AS elem
            WHERE elem LIKE '%:write' AND elem != 'learning-sessions:write'
            UNION ALL
            -- Special: learning-sessions:write → :execute (instead of :update/:delete)
            SELECT 'learning-sessions:execute'
            FROM jsonb_array_elements_text(scopes::jsonb) AS elem
            WHERE elem = 'learning-sessions:write'
            UNION ALL
            -- Special: cp:write also gets :execute
            SELECT 'cp:execute'
            FROM jsonb_array_elements_text(scopes::jsonb) AS elem
            WHERE elem = 'cp:write'
            UNION ALL
            -- Special: mcp:write also gets :execute
            SELECT 'mcp:execute'
            FROM jsonb_array_elements_text(scopes::jsonb) AS elem
            WHERE elem = 'mcp:write'
        ) expanded
        ORDER BY s
    ) ordered
)
WHERE scopes::text LIKE '%:write%';
