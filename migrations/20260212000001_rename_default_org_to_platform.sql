-- Rename the "default" organization to "platform" for existing deployments.
-- New deployments already create "platform" during bootstrap.
-- The WHERE clause ensures idempotency — skips if already renamed.

UPDATE organizations
SET name = 'platform',
    display_name = 'Platform',
    description = 'Platform administration — not a tenant org',
    updated_at = CURRENT_TIMESTAMP
WHERE name = 'default'
  AND NOT EXISTS (SELECT 1 FROM organizations WHERE name = 'platform');
