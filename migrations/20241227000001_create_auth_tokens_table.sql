-- Create personal access token tables
-- Migration: 20241227000001_create_auth_tokens_table.sql

CREATE TABLE IF NOT EXISTS personal_access_tokens (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    token_hash TEXT NOT NULL,
    description TEXT,
    status TEXT NOT NULL DEFAULT 'active',
    expires_at TIMESTAMPTZ,
    last_used_at TIMESTAMPTZ,
    created_by TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS token_scopes (
    id TEXT PRIMARY KEY,
    token_id TEXT NOT NULL,
    scope TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (token_id) REFERENCES personal_access_tokens(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_personal_access_tokens_status
    ON personal_access_tokens(status);

CREATE INDEX IF NOT EXISTS idx_personal_access_tokens_expires_at
    ON personal_access_tokens(expires_at);

CREATE INDEX IF NOT EXISTS idx_token_scopes_token_id
    ON token_scopes(token_id);

CREATE INDEX IF NOT EXISTS idx_token_scopes_scope
    ON token_scopes(scope);
