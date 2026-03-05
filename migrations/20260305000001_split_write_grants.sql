-- Migration: split 'write' action grants into 'create' and 'update'
--
-- For each agent_grants row with action = 'write':
--   1. Insert a new row with action = 'create' (same other fields)
--   2. Update the existing row to action = 'update'
--
-- This is idempotent: the NOT EXISTS guard prevents duplicate rows if run twice.

-- Step 1: Insert 'create' counterpart for each 'write' grant (skip if already exists)
INSERT INTO agent_grants (agent_id, org_id, team, grant_type, resource_type, action, route_id, allowed_methods, created_by, created_at)
SELECT agent_id, org_id, team, grant_type, resource_type, 'create', route_id, allowed_methods, created_by, created_at
FROM agent_grants
WHERE action = 'write'
AND NOT EXISTS (
    SELECT 1 FROM agent_grants g2
    WHERE g2.agent_id = agent_grants.agent_id
    AND g2.org_id = agent_grants.org_id
    AND g2.team = agent_grants.team
    AND g2.grant_type = agent_grants.grant_type
    AND COALESCE(g2.resource_type, '') = COALESCE(agent_grants.resource_type, '')
    AND g2.action = 'create'
);

-- Step 2: Update remaining 'write' grants to 'update'
UPDATE agent_grants SET action = 'update' WHERE action = 'write';
