-- Migration: Add filters scopes
-- Purpose: Add missing filters:read, filters:write, and filters:delete scopes
--
-- The filters API endpoints require these scopes but they were not included
-- in the original scopes migration. This enables proper authorization for:
-- - GET /api/v1/filters (filters:read)
-- - POST /api/v1/filters (filters:write)
-- - GET /api/v1/filters/{id} (filters:read)
-- - PUT /api/v1/filters/{id} (filters:write)
-- - DELETE /api/v1/filters/{id} (filters:write)

INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled)
VALUES
    ('scope-filters-read', 'filters:read', 'filters', 'read', 'Read filters', 'View filter configurations', 'Filters', TRUE, TRUE),
    ('scope-filters-write', 'filters:write', 'filters', 'write', 'Create/update filters', 'Create and modify filters', 'Filters', TRUE, TRUE),
    ('scope-filters-delete', 'filters:delete', 'filters', 'delete', 'Delete filters', 'Remove filter configurations', 'Filters', TRUE, TRUE);
