-- Migration: Add missing authorization scopes
--
-- These resources have API handlers using require_resource_access()
-- but were never seeded in the scopes table. This prevents fine-grained
-- access control - only admin:all can access these endpoints.
--
-- Resources added:
-- - secrets (SDS management)
-- - proxy-certificates (mTLS certificate management)
-- - custom-wasm-filters (custom WASM filter management)
-- - learning-sessions (API learning feature)
-- - aggregated-schemas (API catalog)
-- - reports (analytics)

-- Secrets (SDS) scopes
INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled)
VALUES
    ('scope-secrets-read', 'secrets:read', 'secrets', 'read', 'Read secrets', 'View secret metadata (names, types, references)', 'Secrets', TRUE, TRUE),
    ('scope-secrets-write', 'secrets:write', 'secrets', 'write', 'Create/update secrets', 'Create and modify secret values', 'Secrets', TRUE, TRUE),
    ('scope-secrets-delete', 'secrets:delete', 'secrets', 'delete', 'Delete secrets', 'Remove secrets from the system', 'Secrets', TRUE, TRUE);

-- Proxy Certificates scopes
-- Note: Handler uses 'create' action for POST, not 'write'
INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled)
VALUES
    ('scope-proxy-certificates-read', 'proxy-certificates:read', 'proxy-certificates', 'read', 'Read certificates', 'View proxy certificate metadata', 'Certificates', TRUE, TRUE),
    ('scope-proxy-certificates-create', 'proxy-certificates:create', 'proxy-certificates', 'create', 'Create certificates', 'Generate new proxy certificates for mTLS', 'Certificates', TRUE, TRUE),
    ('scope-proxy-certificates-delete', 'proxy-certificates:delete', 'proxy-certificates', 'delete', 'Revoke certificates', 'Revoke proxy certificates', 'Certificates', TRUE, TRUE);

-- Custom WASM Filters scopes
INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled)
VALUES
    ('scope-custom-wasm-filters-read', 'custom-wasm-filters:read', 'custom-wasm-filters', 'read', 'Read custom filters', 'View custom WASM filter configurations', 'Custom Filters', TRUE, TRUE),
    ('scope-custom-wasm-filters-write', 'custom-wasm-filters:write', 'custom-wasm-filters', 'write', 'Create/update custom filters', 'Upload and modify custom WASM filters', 'Custom Filters', TRUE, TRUE),
    ('scope-custom-wasm-filters-delete', 'custom-wasm-filters:delete', 'custom-wasm-filters', 'delete', 'Delete custom filters', 'Remove custom WASM filters', 'Custom Filters', TRUE, TRUE);

-- Learning Sessions scopes
INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled)
VALUES
    ('scope-learning-sessions-read', 'learning-sessions:read', 'learning-sessions', 'read', 'Read learning sessions', 'View API learning session status and results', 'Learning', TRUE, TRUE),
    ('scope-learning-sessions-write', 'learning-sessions:write', 'learning-sessions', 'write', 'Manage learning sessions', 'Start, stop, and configure learning sessions', 'Learning', TRUE, TRUE),
    ('scope-learning-sessions-delete', 'learning-sessions:delete', 'learning-sessions', 'delete', 'Delete learning sessions', 'Remove learning session data', 'Learning', TRUE, TRUE);

-- Aggregated Schemas scopes (read-only resource)
INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled)
VALUES
    ('scope-aggregated-schemas-read', 'aggregated-schemas:read', 'aggregated-schemas', 'read', 'Read API schemas', 'View learned API schemas', 'Schemas', TRUE, TRUE);

-- Aggregated Schemas scopes (write-only resource)
INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled)
VALUES
    ('scope-aggregated-schemas-write', 'aggregated-schemas:write', 'aggregated-schemas', 'write', 'Export API schemas', 'Export learned API schemas', 'Schemas', TRUE, TRUE);


-- Reports scopes (read-only resource)
INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled)
VALUES
    ('scope-reports-read', 'reports:read', 'reports', 'read', 'Read reports', 'View route flow and analytics reports', 'Reports', TRUE, TRUE);
