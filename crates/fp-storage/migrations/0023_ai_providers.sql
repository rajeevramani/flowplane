-- 0023: S10 AI provider resources. Credentials stay in secrets; this table stores references.

CREATE TABLE ai_providers (
    id                   UUID PRIMARY KEY,
    team_id              UUID NOT NULL,
    org_id               UUID NOT NULL,
    name                 TEXT NOT NULL,
    kind                 TEXT NOT NULL CHECK (kind IN ('openai', 'openai-compatible')),
    base_url             TEXT NOT NULL,
    path_prefix          TEXT,
    credential_secret_id UUID NOT NULL,
    models               TEXT[] NOT NULL DEFAULT '{}',
    auth_header          TEXT NOT NULL DEFAULT 'authorization',
    version              BIGINT NOT NULL DEFAULT 1,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (team_id, name),
    UNIQUE (id, team_id),
    FOREIGN KEY (team_id, org_id) REFERENCES teams(id, org_id) ON DELETE CASCADE,
    FOREIGN KEY (credential_secret_id, team_id) REFERENCES secrets(id, team_id) ON DELETE RESTRICT
);

CREATE INDEX idx_ai_providers_team ON ai_providers(team_id);
