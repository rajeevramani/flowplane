-- Add user_type column to distinguish human from machine users
ALTER TABLE users ADD COLUMN user_type TEXT NOT NULL DEFAULT 'human';

-- Index for listing machine users by org
CREATE INDEX idx_users_user_type ON users(user_type);
