-- Add missing index on users.org_id for efficient org-scoped user queries
CREATE INDEX IF NOT EXISTS idx_users_org_id ON users(org_id);
