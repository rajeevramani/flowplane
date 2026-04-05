-- Drop vestigial users.org_id column
-- In auth v3, org membership is tracked via org_memberships table, not users.org_id.

-- Drop the foreign key constraint
ALTER TABLE users DROP CONSTRAINT IF EXISTS fk_users_org;

-- Drop the index
DROP INDEX IF EXISTS idx_users_org_id;

-- Drop the column
ALTER TABLE users DROP COLUMN IF EXISTS org_id;
