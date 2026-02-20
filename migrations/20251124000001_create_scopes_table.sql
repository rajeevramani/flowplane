-- Create scopes table for database-driven scope registry
-- Migration: 20251124000001_create_scopes_table.sql
--
-- This table stores all valid authorization scopes:
-- - Replaces hardcoded regex validation with semantic whitelist
-- - Provides metadata for UI display (label, description, category)
-- - Enables runtime scope management without code changes
-- - Exposes valid scopes via API for frontend consumption

CREATE TABLE IF NOT EXISTS scopes (
    id TEXT PRIMARY KEY,                           -- UUID for scope
    value TEXT NOT NULL UNIQUE,                    -- The scope string (e.g., "tokens:read")
    resource TEXT NOT NULL,                        -- Resource name (e.g., "tokens")
    action TEXT NOT NULL,                          -- Action name (e.g., "read")
    label TEXT NOT NULL,                           -- Human-readable label for UI
    description TEXT,                              -- Detailed description for UI
    category TEXT NOT NULL,                        -- Category for UI grouping
    visible_in_ui BOOLEAN NOT NULL DEFAULT TRUE,   -- Whether to show in scope selector
    enabled BOOLEAN NOT NULL DEFAULT TRUE,         -- Whether scope is active
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(resource, action)
);

-- Index on value for fast validation lookups
CREATE INDEX IF NOT EXISTS idx_scopes_value ON scopes(value);

-- Index on resource for listing scopes by resource
CREATE INDEX IF NOT EXISTS idx_scopes_resource ON scopes(resource);

-- Index on enabled for filtering active scopes
CREATE INDEX IF NOT EXISTS idx_scopes_enabled ON scopes(enabled);

-- Index on category for UI grouping
CREATE INDEX IF NOT EXISTS idx_scopes_category ON scopes(category);

-- Composite index for common query pattern (enabled + visible)
CREATE INDEX IF NOT EXISTS idx_scopes_enabled_visible ON scopes(enabled, visible_in_ui);

-- Seed default scopes for all resources with read/write/delete actions
-- Resources: tokens, clusters, routes, listeners, openapi-import, generate-envoy-config

-- Tokens
INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled)
VALUES
    ('scope-tokens-read', 'tokens:read', 'tokens', 'read', 'Read tokens', 'View and list personal access tokens', 'Tokens', TRUE, TRUE),
    ('scope-tokens-write', 'tokens:write', 'tokens', 'write', 'Create/update tokens', 'Create and modify personal access tokens', 'Tokens', TRUE, TRUE),
    ('scope-tokens-delete', 'tokens:delete', 'tokens', 'delete', 'Delete tokens', 'Revoke and delete personal access tokens', 'Tokens', TRUE, TRUE);

-- Clusters
INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled)
VALUES
    ('scope-clusters-read', 'clusters:read', 'clusters', 'read', 'Read clusters', 'View cluster configurations', 'Clusters', TRUE, TRUE),
    ('scope-clusters-write', 'clusters:write', 'clusters', 'write', 'Create/update clusters', 'Create and modify clusters', 'Clusters', TRUE, TRUE),
    ('scope-clusters-delete', 'clusters:delete', 'clusters', 'delete', 'Delete clusters', 'Remove cluster configurations', 'Clusters', TRUE, TRUE);

-- Routes
INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled)
VALUES
    ('scope-routes-read', 'routes:read', 'routes', 'read', 'Read routes', 'View route configurations', 'Routes', TRUE, TRUE),
    ('scope-routes-write', 'routes:write', 'routes', 'write', 'Create/update routes', 'Create and modify routes', 'Routes', TRUE, TRUE),
    ('scope-routes-delete', 'routes:delete', 'routes', 'delete', 'Delete routes', 'Remove route configurations', 'Routes', TRUE, TRUE);

-- Listeners
INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled)
VALUES
    ('scope-listeners-read', 'listeners:read', 'listeners', 'read', 'Read listeners', 'View listener configurations', 'Listeners', TRUE, TRUE),
    ('scope-listeners-write', 'listeners:write', 'listeners', 'write', 'Create/update listeners', 'Create and modify listeners', 'Listeners', TRUE, TRUE),
    ('scope-listeners-delete', 'listeners:delete', 'listeners', 'delete', 'Delete listeners', 'Remove listener configurations', 'Listeners', TRUE, TRUE);

-- OpenAPI Import
INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled)
VALUES
    ('scope-openapi-import-read', 'openapi-import:read', 'openapi-import', 'read', 'View imports', 'View OpenAPI import history', 'OpenAPI Import', TRUE, TRUE),
    ('scope-openapi-import-write', 'openapi-import:write', 'openapi-import', 'write', 'Create imports', 'Import OpenAPI specifications', 'OpenAPI Import', TRUE, TRUE),
    ('scope-openapi-import-delete', 'openapi-import:delete', 'openapi-import', 'delete', 'Delete imports', 'Remove OpenAPI imports', 'OpenAPI Import', TRUE, TRUE);

-- Generate Envoy Config (read-only resource)
INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled)
VALUES
    ('scope-generate-envoy-config-read', 'generate-envoy-config:read', 'generate-envoy-config', 'read', 'Generate bootstrap', 'Generate Envoy bootstrap configuration', 'Envoy Config', TRUE, TRUE);

-- Admin scope (not visible in UI, for super admin operations)
INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled)
VALUES
    ('scope-admin-all', 'admin:all', 'admin', 'all', 'Full admin access', 'Grants full administrative access to all resources', 'Admin', FALSE, TRUE);
