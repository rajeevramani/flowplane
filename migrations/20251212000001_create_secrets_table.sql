-- Create secrets table for SDS (Secret Discovery Service)
-- Secrets are encrypted at rest using AES-256-GCM

CREATE TABLE IF NOT EXISTS secrets (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    secret_type TEXT NOT NULL CHECK (secret_type IN (
        'generic_secret',
        'tls_certificate',
        'certificate_validation_context',
        'session_ticket_keys'
    )),
    description TEXT,
    -- Encrypted secret configuration (JSON encrypted with AES-256-GCM)
    configuration_encrypted BYTEA NOT NULL,
    -- Encryption key identifier for key rotation support
    encryption_key_id TEXT NOT NULL DEFAULT 'default',
    -- Nonce used for AES-GCM encryption (unique per secret)
    nonce BYTEA NOT NULL,
    -- Version for optimistic locking and xDS versioning
    version BIGINT NOT NULL DEFAULT 1,
    -- Source API that created this secret
    source TEXT NOT NULL DEFAULT 'native_api' CHECK (source IN ('native_api', 'gateway_api', 'platform_api')),
    -- Team ownership for multi-tenancy
    team TEXT NOT NULL,
    -- Timestamps
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    -- Optional expiration for time-limited secrets
    expires_at TIMESTAMPTZ,

    FOREIGN KEY (team) REFERENCES teams(name) ON DELETE RESTRICT,
    UNIQUE(team, name)
);

-- Index for team-scoped queries (most common access pattern)
CREATE INDEX idx_secrets_team ON secrets(team);

-- Index for team + name lookups
CREATE INDEX idx_secrets_team_name ON secrets(team, name);

-- Index for secret type filtering
CREATE INDEX idx_secrets_type ON secrets(secret_type);

-- Index for change detection in xDS watchers
CREATE INDEX idx_secrets_updated_at ON secrets(updated_at);

-- Index for expiration-based queries (secret rotation reminders)
CREATE INDEX idx_secrets_expires_at ON secrets(expires_at) WHERE expires_at IS NOT NULL;
