-- Migration: split 'write' action grants into 'create' and 'update'
--
-- For each agent_grants row with action = 'write':
--   1. Insert a new row with action = 'create' (same other fields)
--   2. Update the existing row to action = 'update'
--
-- Idempotent: INSERT uses ON CONFLICT DO NOTHING against the cp-tool unique index,
-- and the UPDATE is a no-op when no 'write' rows remain.

-- Step 1: Insert 'create' counterpart for each 'write' cp-tool grant
INSERT INTO agent_grants (id, agent_id, org_id, team, grant_type, resource_type, action, route_id, allowed_methods, created_by, created_at, expires_at)
SELECT
    md5(ag.id || '-create-split') AS id,
    ag.agent_id, ag.org_id, ag.team, ag.grant_type, ag.resource_type,
    'create',
    ag.route_id, ag.allowed_methods, ag.created_by, ag.created_at, ag.expires_at
FROM agent_grants ag
WHERE ag.action = 'write'
AND ag.grant_type = 'cp-tool'
ON CONFLICT DO NOTHING;

-- Step 2: Update remaining 'write' grants to 'update'
UPDATE agent_grants SET action = 'update' WHERE action = 'write';
