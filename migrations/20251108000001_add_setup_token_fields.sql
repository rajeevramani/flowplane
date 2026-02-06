-- Add setup token support to personal_access_tokens table
-- Migration: 20251108000001_add_setup_token_fields.sql

-- Add is_setup_token column to identify setup tokens
ALTER TABLE personal_access_tokens
ADD COLUMN is_setup_token BOOLEAN NOT NULL DEFAULT FALSE;

-- Add max_usage_count column to limit the number of times a setup token can be used
ALTER TABLE personal_access_tokens
ADD COLUMN max_usage_count BIGINT;

-- Add usage_count column to track how many times a setup token has been used
ALTER TABLE personal_access_tokens
ADD COLUMN usage_count BIGINT NOT NULL DEFAULT 0;

-- Create index for setup tokens to optimize queries filtering by is_setup_token
CREATE INDEX IF NOT EXISTS idx_personal_access_tokens_is_setup_token
    ON personal_access_tokens(is_setup_token);

-- Create composite index for finding active, non-expired setup tokens
CREATE INDEX IF NOT EXISTS idx_personal_access_tokens_setup_active
    ON personal_access_tokens(is_setup_token, status, expires_at)
    WHERE is_setup_token = TRUE;
