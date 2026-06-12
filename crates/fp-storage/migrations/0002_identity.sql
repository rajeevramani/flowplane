-- 0002: identity & tenancy backbone (spec/10 §4, spec/05, spec/08a).
-- Conventions locked here for every later table:
--   * team column is always `team_id` (v1's name/id confusion caused real bugs — spec/03 §8.2)
--   * (org_id, team_id) composite FK proves team ∈ org at the schema level (spec/08a §2.2.9)
--   * per-tenant name uniqueness, never global (spec/08a §2.2.2)
--   * status/role/kind columns carry CHECK constraints (v1 left them free-text — spec/03 §8.13)

CREATE TABLE organizations (
    id           UUID PRIMARY KEY,
    name         TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL DEFAULT '',
    status       TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'suspended')),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE teams (
    id           UUID PRIMARY KEY,
    org_id       UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    name         TEXT NOT NULL,
    display_name TEXT NOT NULL DEFAULT '',
    status       TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'suspended')),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (org_id, name),
    -- Referenced by composite FKs that prove team ∈ org (grants and, later, every
    -- team-owned resource table).
    UNIQUE (id, org_id)
);

-- Human identities. Provisioned on first authenticated request (JIT) or by invite;
-- `subject` is the OIDC `sub` claim — provider-agnostic (Q-004). No password storage, ever.
CREATE TABLE users (
    id         UUID PRIMARY KEY,
    subject    TEXT NOT NULL UNIQUE,
    email      TEXT NOT NULL DEFAULT '',
    name       TEXT NOT NULL DEFAULT '',
    status     TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'suspended')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Machine identities, org-owned. `kind` structurally partitions what surfaces an agent can
-- ever reach (spec/05 §3: cp-tool / gateway-tool / api-consumer).
CREATE TABLE agents (
    id           UUID PRIMARY KEY,
    org_id       UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    kind         TEXT NOT NULL CHECK (kind IN ('cp-tool', 'gateway-tool', 'api-consumer')),
    token_hash   TEXT NOT NULL,
    status       TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'suspended')),
    created_by   UUID REFERENCES users(id) ON DELETE SET NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (org_id, name)
);

CREATE TABLE org_memberships (
    id         UUID PRIMARY KEY,
    user_id    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    org_id     UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    role       TEXT NOT NULL CHECK (role IN ('owner', 'admin', 'member', 'viewer')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, org_id)
);

CREATE TABLE team_memberships (
    id         UUID PRIMARY KEY,
    user_id    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    team_id    UUID NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, team_id)
);

-- The grant table: (principal × resource × action × team). The composite FK makes a grant
-- whose team is outside its org unrepresentable.
CREATE TABLE grants (
    id             UUID PRIMARY KEY,
    principal_type TEXT NOT NULL CHECK (principal_type IN ('user', 'agent')),
    principal_id   UUID NOT NULL,
    org_id         UUID NOT NULL,
    team_id        UUID NOT NULL,
    resource       TEXT NOT NULL,
    action         TEXT NOT NULL CHECK (action IN ('read', 'create', 'update', 'delete', 'execute')),
    created_by     UUID REFERENCES users(id) ON DELETE SET NULL,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (team_id, org_id) REFERENCES teams(id, org_id) ON DELETE CASCADE,
    UNIQUE (principal_type, principal_id, team_id, resource, action)
);
CREATE INDEX idx_grants_principal ON grants(principal_type, principal_id);
CREATE INDEX idx_grants_team ON grants(team_id);

-- Audit log: deliberately no FKs (rows must survive subject deletion — spec/03 §8.17),
-- but unlike v1 it has a retention policy hook (occurred_at index) and covers denials.
CREATE TABLE audit_log (
    id          UUID PRIMARY KEY,
    occurred_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    request_id  UUID,
    actor_type  TEXT NOT NULL CHECK (actor_type IN ('user', 'agent', 'dataplane', 'system', 'anonymous')),
    actor_id    UUID,
    actor_label TEXT NOT NULL DEFAULT '',
    surface     TEXT NOT NULL CHECK (surface IN ('rest', 'mcp', 'cli', 'xds', 'system')),
    action      TEXT NOT NULL,
    resource    TEXT NOT NULL DEFAULT '',
    org_id      UUID,
    team_id     UUID,
    outcome     TEXT NOT NULL CHECK (outcome IN ('success', 'denied', 'failure')),
    detail      JSONB NOT NULL DEFAULT '{}'::jsonb
);
CREATE INDEX idx_audit_occurred ON audit_log(occurred_at);
CREATE INDEX idx_audit_org ON audit_log(org_id, occurred_at);
CREATE INDEX idx_audit_team ON audit_log(team_id, occurred_at);

-- One-shot bootstrap tokens (spec/08a §2.2.10): hashed at rest, expiring, single-use.
CREATE TABLE bootstrap_tokens (
    id         UUID PRIMARY KEY,
    token_hash TEXT NOT NULL UNIQUE,
    expires_at TIMESTAMPTZ NOT NULL,
    used_at    TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
