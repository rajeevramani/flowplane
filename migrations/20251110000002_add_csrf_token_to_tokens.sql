-- Add CSRF token column to personal access tokens table
-- Migration: 20251110000002_add_csrf_token_to_tokens.sql

ALTER TABLE personal_access_tokens ADD COLUMN csrf_token TEXT;

-- Index for CSRF token lookups
CREATE INDEX IF NOT EXISTS idx_personal_access_tokens_csrf_token
    ON personal_access_tokens(csrf_token);
