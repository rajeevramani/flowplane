-- Add token_type column to distinguish between PAT, session, and setup tokens
-- Migration: 20251110000001_add_token_type_column.sql

-- Add token_type column with default value 'pat' for existing tokens
ALTER TABLE personal_access_tokens
ADD COLUMN token_type TEXT NOT NULL DEFAULT 'pat';

-- Create index on token_type for efficient querying
CREATE INDEX IF NOT EXISTS idx_personal_access_tokens_token_type
    ON personal_access_tokens(token_type);

-- Create composite index for filtering active tokens by type
CREATE INDEX IF NOT EXISTS idx_personal_access_tokens_type_status
    ON personal_access_tokens(token_type, status);
