-- Add org_id and team_id columns to audit_log for multi-tenancy observability
-- Migration: 20260207000003_add_org_team_to_audit_log.sql

ALTER TABLE audit_log ADD COLUMN IF NOT EXISTS org_id TEXT;
ALTER TABLE audit_log ADD COLUMN IF NOT EXISTS team_id TEXT;

-- Index for org-based audit queries
CREATE INDEX IF NOT EXISTS idx_audit_org ON audit_log(org_id);

-- Index for team-based audit queries
CREATE INDEX IF NOT EXISTS idx_audit_team ON audit_log(team_id);
