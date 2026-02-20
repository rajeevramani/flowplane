-- Add governance-specific admin scopes for security model hardening
-- Migration: 20260214000001_add_governance_scopes.sql
--
-- Previously `admin:all` was a universal bypass granting full access to all resources.
-- Now `admin:all` is restricted to platform governance only (orgs, users, audit, summary).
-- These new scopes make the governance permissions explicit and granular.

-- Update admin:all description to reflect new restricted meaning
UPDATE scopes
SET label = 'Platform governance',
    description = 'Platform governance access — org management, user management, audit logs, admin summary. Does NOT grant access to tenant resources.'
WHERE value = 'admin:all';

-- Organization management scopes
INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled)
VALUES
    ('scope-admin-orgs-read', 'admin:orgs:read', 'admin-orgs', 'read', 'Read organizations', 'View organization details and list all organizations', 'Admin', FALSE, TRUE),
    ('scope-admin-orgs-write', 'admin:orgs:write', 'admin-orgs', 'write', 'Manage organizations', 'Create and update organizations', 'Admin', FALSE, TRUE),
    ('scope-admin-orgs-delete', 'admin:orgs:delete', 'admin-orgs', 'delete', 'Delete organizations', 'Remove organizations from the platform', 'Admin', FALSE, TRUE);

-- User management scopes
INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled)
VALUES
    ('scope-admin-users-read', 'admin:users:read', 'admin-users', 'read', 'Read users', 'View user accounts and list all users', 'Admin', FALSE, TRUE),
    ('scope-admin-users-write', 'admin:users:write', 'admin-users', 'write', 'Manage users', 'Create and update user accounts', 'Admin', FALSE, TRUE),
    ('scope-admin-users-delete', 'admin:users:delete', 'admin-users', 'delete', 'Delete users', 'Remove user accounts from the platform', 'Admin', FALSE, TRUE);

-- Team management scopes (admin-level, for /api/v1/admin/teams endpoints)
INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled)
VALUES
    ('scope-admin-teams-read', 'admin:teams:read', 'admin-teams', 'read', 'Read teams (admin)', 'View and list all teams across all organizations', 'Admin', FALSE, TRUE),
    ('scope-admin-teams-write', 'admin:teams:write', 'admin-teams', 'write', 'Manage teams (admin)', 'Create and update teams via admin endpoints', 'Admin', FALSE, TRUE),
    ('scope-admin-teams-delete', 'admin:teams:delete', 'admin-teams', 'delete', 'Delete teams (admin)', 'Remove teams via admin endpoints', 'Admin', FALSE, TRUE);

-- Audit log scope
INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled)
VALUES
    ('scope-admin-audit-read', 'admin:audit:read', 'admin-audit', 'read', 'Read audit logs', 'View platform audit trail and security events', 'Admin', FALSE, TRUE);

-- Admin summary/dashboard scope
INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled)
VALUES
    ('scope-admin-summary-read', 'admin:summary:read', 'admin-summary', 'read', 'View admin summary', 'Access admin dashboard with aggregate resource counts', 'Admin', FALSE, TRUE);

-- Admin scopes scope (for /api/v1/admin/scopes endpoint)
INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled)
VALUES
    ('scope-admin-scopes-read', 'admin:scopes:read', 'admin-scopes', 'read', 'Read scopes registry', 'View all available authorization scopes', 'Admin', FALSE, TRUE);

-- Admin apps scope (for /api/v1/admin/apps endpoints)
INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled)
VALUES
    ('scope-admin-apps-read', 'admin:apps:read', 'admin-apps', 'read', 'Read instance apps', 'View instance application configurations', 'Admin', FALSE, TRUE),
    ('scope-admin-apps-write', 'admin:apps:write', 'admin-apps', 'write', 'Manage instance apps', 'Update instance application configurations', 'Admin', FALSE, TRUE);

-- Admin filter schemas scope
INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled)
VALUES
    ('scope-admin-filter-schemas-write', 'admin:filter-schemas:write', 'admin-filter-schemas', 'write', 'Reload filter schemas', 'Reload filter schema definitions from disk', 'Admin', FALSE, TRUE);
