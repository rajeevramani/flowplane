INSERT INTO scopes (id, value, resource, action, label, description, category, visible_in_ui, enabled)
VALUES
    ('scope-agents-read', 'agents:read', 'agents', 'read',
     'Read agents', 'List agents in your team', 'Resources', TRUE, TRUE),
    ('scope-agents-write', 'agents:write', 'agents', 'write',
     'Manage agents', 'Create, update, and delete agents in your team', 'Resources', TRUE, TRUE)
ON CONFLICT (id) DO NOTHING;
