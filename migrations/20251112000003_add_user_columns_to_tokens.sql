-- Add user_id and user_email columns to personal_access_tokens table
-- Migration: 20251112000003_add_user_columns_to_tokens.sql

-- Add user_id column (nullable for backward compatibility with existing tokens)
ALTER TABLE personal_access_tokens
ADD COLUMN user_id TEXT;

-- Add user_email column (nullable for backward compatibility)
ALTER TABLE personal_access_tokens
ADD COLUMN user_email TEXT;

-- Add foreign key constraint to users table
-- PostgreSQL supports adding FK constraints to existing tables
ALTER TABLE personal_access_tokens
    ADD CONSTRAINT fk_personal_access_tokens_user_id
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE SET NULL;

-- Index for looking up all tokens for a user
CREATE INDEX IF NOT EXISTS idx_personal_access_tokens_user_id
    ON personal_access_tokens(user_id);

-- Index for user_email lookups
CREATE INDEX IF NOT EXISTS idx_personal_access_tokens_user_email
    ON personal_access_tokens(user_email);

-- Composite index for finding active tokens by user
CREATE INDEX IF NOT EXISTS idx_personal_access_tokens_user_status
    ON personal_access_tokens(user_id, status)
    WHERE user_id IS NOT NULL;
