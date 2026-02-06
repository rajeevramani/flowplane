-- Add security fields for setup token protection
-- Migration: 20251109000001_add_setup_token_security_fields.sql

-- Add failed_attempts column to track failed authentication attempts
ALTER TABLE personal_access_tokens
ADD COLUMN failed_attempts BIGINT NOT NULL DEFAULT 0;

-- Add locked_until column to implement temporary lockout after failed attempts
ALTER TABLE personal_access_tokens
ADD COLUMN locked_until TIMESTAMP;

-- Create index for locked tokens to optimize queries
CREATE INDEX IF NOT EXISTS idx_personal_access_tokens_locked_until
    ON personal_access_tokens(locked_until)
    WHERE locked_until IS NOT NULL;

-- Create composite index for finding setup tokens that are not locked
CREATE INDEX IF NOT EXISTS idx_personal_access_tokens_setup_unlocked
    ON personal_access_tokens(is_setup_token, locked_until)
    WHERE is_setup_token = TRUE;
