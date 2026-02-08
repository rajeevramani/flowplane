-- Enforce org_id NOT NULL on users and teams, and make team names org-scoped
--
-- This migration:
-- 1. Backfills any NULL org_id rows with the default organization
-- 2. Makes org_id NOT NULL on both users and teams tables
-- 3. Changes team name uniqueness from global to org-scoped: UNIQUE(org_id, name)
--
-- Prerequisites: A 'default' organization must exist (created by bootstrap).
-- If no default org exists and NULL rows are present, the migration will fail safely.

-- Step 1: Backfill NULL org_id on users
UPDATE users
SET org_id = (SELECT id FROM organizations WHERE name = 'default' LIMIT 1)
WHERE org_id IS NULL
  AND EXISTS (SELECT 1 FROM organizations WHERE name = 'default');

-- Step 2: Backfill NULL org_id on teams
UPDATE teams
SET org_id = (SELECT id FROM organizations WHERE name = 'default' LIMIT 1)
WHERE org_id IS NULL
  AND EXISTS (SELECT 1 FROM organizations WHERE name = 'default');

-- Step 3: Validate no NULLs remain before applying NOT NULL constraint
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM users WHERE org_id IS NULL) THEN
        RAISE EXCEPTION 'Cannot enforce NOT NULL: users table still has rows with NULL org_id. Run bootstrap first.';
    END IF;
    IF EXISTS (SELECT 1 FROM teams WHERE org_id IS NULL) THEN
        RAISE EXCEPTION 'Cannot enforce NOT NULL: teams table still has rows with NULL org_id. Run bootstrap first.';
    END IF;
END $$;

-- Step 4: Make org_id NOT NULL
ALTER TABLE users ALTER COLUMN org_id SET NOT NULL;
ALTER TABLE teams ALTER COLUMN org_id SET NOT NULL;

-- Step 5: Drop global team name unique constraint and index
ALTER TABLE teams DROP CONSTRAINT IF EXISTS teams_name_key;
DROP INDEX IF EXISTS idx_teams_name;

-- Step 6: Add org-scoped team name uniqueness
ALTER TABLE teams ADD CONSTRAINT teams_org_name_key UNIQUE(org_id, name);
CREATE INDEX IF NOT EXISTS idx_teams_org_name ON teams(org_id, name);

-- Step 7: Keep the status+name index but make it org-scoped too
DROP INDEX IF EXISTS idx_teams_status_name;
CREATE INDEX IF NOT EXISTS idx_teams_status_org_name ON teams(status, org_id, name);
