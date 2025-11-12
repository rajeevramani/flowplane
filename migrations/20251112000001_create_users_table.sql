-- Create users table for authentication and authorization
-- Migration: 20251112000001_create_users_table.sql

CREATE TABLE IF NOT EXISTS users (
    id TEXT PRIMARY KEY,
    email TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    name TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active',
    is_admin BOOLEAN NOT NULL DEFAULT FALSE,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- Index for email lookups (used during login)
CREATE INDEX IF NOT EXISTS idx_users_email
    ON users(email);

-- Index for status filtering (e.g., finding active users)
CREATE INDEX IF NOT EXISTS idx_users_status
    ON users(status);

-- Index for admin filtering
CREATE INDEX IF NOT EXISTS idx_users_is_admin
    ON users(is_admin);

-- Composite index for finding active admins
CREATE INDEX IF NOT EXISTS idx_users_status_admin
    ON users(status, is_admin)
    WHERE is_admin = TRUE;
