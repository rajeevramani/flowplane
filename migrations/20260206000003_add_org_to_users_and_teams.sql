-- Add org_id to users (DO NOT rename is_admin - it means platform superadmin)
ALTER TABLE users ADD COLUMN org_id TEXT;
ALTER TABLE users ADD CONSTRAINT fk_users_org
    FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE RESTRICT;

-- Add org_id to teams
ALTER TABLE teams ADD COLUMN org_id TEXT;
ALTER TABLE teams ADD CONSTRAINT fk_teams_org
    FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE RESTRICT;
CREATE INDEX idx_teams_org_id ON teams(org_id);
