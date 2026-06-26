-- 0030: S1 first-party global rate-limit policy model (spec/03 §3.7, feature fpv2-4ht).
--
-- Three team-owned, org-trigger-guarded tables: domains -> policies -> team_overrides
-- (FK chain). Team-in-org is enforced by the composite FK to teams(id, org_id) — the same
-- mechanism every tenant table uses (e.g. 0027 ai_budgets), not a bespoke trigger.
--
-- These are the one place Flowplane keeps SOFT DELETE (spec/03 §3.7): a `deleted_at`
-- tombstone plus partial-unique indexes that only constrain live rows, so a deleted name
-- can be reused. Optimistic concurrency via `version` on every mutable row.
--
-- descriptors_canonical is computed in Rust (fp-domain) before insert — the deterministic
-- sorted-key form of the descriptor map (spec/03:485) — and stored here as the match key.

-- Reserved-name guard, pre-existing rows (acceptance #7). The built-in CDS cluster injected
-- in S6 is named `rate_limit_cluster`; the `rate_limit_` prefix is reserved. Clusters are only
-- UNIQUE(team_id, name) (0004_clusters.sql), so a pre-existing collision must fail the
-- migration closed. No production data exists yet, so this is the simplest correct guard.
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM clusters WHERE name LIKE 'rate\_limit\_%' ESCAPE '\') THEN
        RAISE EXCEPTION 'the "rate_limit_" cluster-name prefix is reserved for the built-in '
            'rate-limit cluster (feature fpv2-4ht); rename the conflicting cluster(s) before '
            'applying migration 0030';
    END IF;
END $$;

-- A rate-limit domain: the user-facing group an operator names their limits under. `name`
-- carries the policy `domain` string (1–253 chars, spec/02:329) and is the CRUD handle.
CREATE TABLE rate_limit_domains (
    id          UUID PRIMARY KEY,
    team_id     UUID NOT NULL,
    org_id      UUID NOT NULL,
    name        TEXT NOT NULL CHECK (char_length(name) BETWEEN 1 AND 253),
    version     BIGINT NOT NULL DEFAULT 1,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at  TIMESTAMPTZ,
    UNIQUE (id, team_id),
    FOREIGN KEY (team_id, org_id) REFERENCES teams(id, org_id) ON DELETE CASCADE
);
-- Name is unique only among live rows, so a soft-deleted name can be recreated.
CREATE UNIQUE INDEX uq_rate_limit_domains_team_name
    ON rate_limit_domains (team_id, name) WHERE deleted_at IS NULL;
CREATE INDEX idx_rate_limit_domains_team ON rate_limit_domains (team_id);

-- A policy: one descriptor-set -> requests_per_unit + unit, inside a domain. `name` is the
-- CRUD handle; descriptors_canonical is the deterministic match key (computed in Rust).
CREATE TABLE rate_limit_policies (
    id                    UUID PRIMARY KEY,
    team_id               UUID NOT NULL,
    org_id                UUID NOT NULL,
    domain_id             UUID NOT NULL,
    name                  TEXT NOT NULL,
    descriptors           JSONB NOT NULL,
    descriptors_canonical TEXT NOT NULL,
    requests_per_unit     BIGINT NOT NULL CHECK (requests_per_unit > 0),
    unit                  TEXT NOT NULL CHECK (unit IN ('second', 'minute', 'hour', 'day')),
    version               BIGINT NOT NULL DEFAULT 1,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at            TIMESTAMPTZ,
    UNIQUE (id, team_id),
    FOREIGN KEY (team_id, org_id) REFERENCES teams(id, org_id) ON DELETE CASCADE,
    FOREIGN KEY (domain_id, team_id) REFERENCES rate_limit_domains(id, team_id) ON DELETE CASCADE
);
-- Addressable by name within a domain (live rows only)…
CREATE UNIQUE INDEX uq_rate_limit_policies_domain_name
    ON rate_limit_policies (team_id, domain_id, name) WHERE deleted_at IS NULL;
-- …and the descriptor match must be deterministic: no two live policies in a domain share
-- the same canonical descriptor set, so RLS lookup resolves to exactly one policy.
CREATE UNIQUE INDEX uq_rate_limit_policies_match
    ON rate_limit_policies (team_id, domain_id, descriptors_canonical) WHERE deleted_at IS NULL;
CREATE INDEX idx_rate_limit_policies_team ON rate_limit_policies (team_id);

-- A team override: replaces a policy's requests_per_unit for this team. Addressable by id;
-- at most one live override per policy.
CREATE TABLE rate_limit_team_overrides (
    id                UUID PRIMARY KEY,
    team_id           UUID NOT NULL,
    org_id            UUID NOT NULL,
    policy_id         UUID NOT NULL,
    requests_per_unit BIGINT NOT NULL CHECK (requests_per_unit > 0),
    version           BIGINT NOT NULL DEFAULT 1,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at        TIMESTAMPTZ,
    UNIQUE (id, team_id),
    FOREIGN KEY (team_id, org_id) REFERENCES teams(id, org_id) ON DELETE CASCADE,
    FOREIGN KEY (policy_id, team_id) REFERENCES rate_limit_policies(id, team_id) ON DELETE CASCADE
);
CREATE UNIQUE INDEX uq_rate_limit_team_overrides_policy
    ON rate_limit_team_overrides (team_id, policy_id) WHERE deleted_at IS NULL;
CREATE INDEX idx_rate_limit_team_overrides_team ON rate_limit_team_overrides (team_id);
