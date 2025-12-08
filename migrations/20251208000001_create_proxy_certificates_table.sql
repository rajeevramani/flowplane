-- Create proxy_certificates table for mTLS certificate tracking
-- Migration: 20251208000001_create_proxy_certificates_table.sql
--
-- This table tracks certificates issued for Envoy proxies to enable:
-- - Audit trail of certificate issuance
-- - Expiry tracking for renewal notifications
-- - Certificate revocation support
--
-- Note: Private keys are never stored - only issued once at generation time

CREATE TABLE IF NOT EXISTS proxy_certificates (
    id TEXT PRIMARY KEY,                           -- UUID for certificate record
    team_id TEXT NOT NULL,                         -- Team owning this certificate
    proxy_id TEXT NOT NULL,                        -- Unique proxy instance identifier
    serial_number TEXT NOT NULL,                   -- Certificate serial from Vault
    spiffe_uri TEXT NOT NULL,                      -- Full SPIFFE identity URI
    issued_at TEXT NOT NULL,                       -- ISO 8601 timestamp
    expires_at TEXT NOT NULL,                      -- ISO 8601 timestamp
    issued_by_user_id TEXT,                        -- User who generated the certificate
    revoked_at TEXT,                               -- NULL if not revoked
    revoked_reason TEXT,                           -- Reason for revocation
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (team_id) REFERENCES teams(id) ON DELETE CASCADE,
    FOREIGN KEY (issued_by_user_id) REFERENCES users(id) ON DELETE SET NULL,
    UNIQUE(team_id, serial_number)
);

-- Index for listing certificates by team
CREATE INDEX IF NOT EXISTS idx_proxy_certificates_team_id ON proxy_certificates(team_id);

-- Index for finding certificates by expiry (for renewal notifications)
CREATE INDEX IF NOT EXISTS idx_proxy_certificates_expires_at ON proxy_certificates(expires_at);

-- Index for finding certificates by proxy_id within a team
CREATE INDEX IF NOT EXISTS idx_proxy_certificates_team_proxy ON proxy_certificates(team_id, proxy_id);

-- Index for finding non-revoked certificates
CREATE INDEX IF NOT EXISTS idx_proxy_certificates_revoked ON proxy_certificates(revoked_at);
