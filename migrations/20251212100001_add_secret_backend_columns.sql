-- Add columns for reference-based secrets (external backend support)
-- This enables Flowplane to store only references to external secret backends
-- (Vault, AWS Secrets Manager, GCP Secret Manager) instead of storing encrypted values.

-- Backend type: "vault", "aws_secrets_manager", "gcp_secret_manager", "database" (default)
-- NULL means legacy database-stored encrypted secret
ALTER TABLE secrets ADD COLUMN backend TEXT;

-- Backend-specific reference (Vault path, AWS ARN, GCP resource name)
-- NULL means legacy database-stored encrypted secret
ALTER TABLE secrets ADD COLUMN reference TEXT;

-- Optional version specifier for the external secret
ALTER TABLE secrets ADD COLUMN reference_version TEXT;

-- Index for efficient backend queries
CREATE INDEX idx_secrets_backend ON secrets(backend);
