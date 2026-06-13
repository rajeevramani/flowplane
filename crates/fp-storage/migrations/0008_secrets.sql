-- 0008: encrypted-at-rest SDS secrets. Values are never stored as plaintext: the JSON
-- SecretSpec is AES-256-GCM ciphertext plus nonce, keyed by encryption_key_id.

CREATE TABLE secrets (
    id                       UUID PRIMARY KEY,
    team_id                  UUID NOT NULL,
    org_id                   UUID NOT NULL,
    name                     TEXT NOT NULL,
    description              TEXT NOT NULL DEFAULT '',
    secret_type              TEXT NOT NULL CHECK (
        secret_type IN (
            'generic_secret',
            'tls_certificate',
            'certificate_validation_context',
            'session_ticket_keys'
        )
    ),
    configuration_encrypted  BYTEA NOT NULL,
    nonce                    BYTEA NOT NULL,
    encryption_key_id        TEXT NOT NULL DEFAULT 'default',
    version                  BIGINT NOT NULL DEFAULT 1,
    created_at               TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at               TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at               TIMESTAMPTZ,
    UNIQUE (team_id, name),
    UNIQUE (id, team_id),
    FOREIGN KEY (team_id, org_id) REFERENCES teams(id, org_id) ON DELETE CASCADE
);

CREATE INDEX idx_secrets_team ON secrets(team_id);
CREATE INDEX idx_secrets_expires_at ON secrets(expires_at) WHERE expires_at IS NOT NULL;
