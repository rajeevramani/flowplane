-- Adjust audit_log constraints to support auth events
-- Migration: 20250101000002_relax_audit_log_constraints.sql
--
-- PostgreSQL: Use ALTER TABLE instead of SQLite table recreation pattern.
-- Changes:
--   1. resource_id: NOT NULL → nullable
--   2. resource_name: NOT NULL → nullable
--   3. CHECK constraints relaxed from specific values to non-empty checks

-- Make resource_id and resource_name nullable
ALTER TABLE audit_log ALTER COLUMN resource_id DROP NOT NULL;
ALTER TABLE audit_log ALTER COLUMN resource_name DROP NOT NULL;

-- Drop the restrictive CHECK constraints from the original table
ALTER TABLE audit_log DROP CONSTRAINT IF EXISTS audit_log_action_check;
ALTER TABLE audit_log DROP CONSTRAINT IF EXISTS audit_log_resource_type_check;

-- Add relaxed CHECK constraints (allow any non-empty values)
ALTER TABLE audit_log ADD CONSTRAINT audit_log_resource_type_check CHECK (length(trim(resource_type)) > 0);
ALTER TABLE audit_log ADD CONSTRAINT audit_log_action_check CHECK (length(trim(action)) > 0);
