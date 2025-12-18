-- Add stats dashboard scopes
-- This allows team-scoped access to stats endpoints

INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled)
VALUES
    ('scope-stats-read', 'stats:read', 'stats', 'read', 'Read stats', 'View Envoy stats and metrics', 'Stats', TRUE, TRUE);
