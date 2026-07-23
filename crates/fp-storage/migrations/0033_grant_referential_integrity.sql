-- 0033: grant referential integrity — split the polymorphic `grants` table into typed
-- `user_grants` / `agent_grants` so an orphaned or cross-org grant is unrepresentable.
--
-- The old table was polymorphic (`principal_type` + a bare `principal_id` with NO foreign key)
-- precisely because PostgreSQL cannot point one column at two tables (0002_identity.sql:75-92).
-- That left two holes this migration closes by construction:
--   * a grant survived its principal's org membership being removed, so authority outlived
--     the membership it was contingent on;
--   * nothing tied the principal to an org at all, so an agent grant could name an agent in
--     a different org than the row claimed.
-- Splitting by principal kind lets each table declare real composite FKs. `org_id` is shared
-- between each table's two FKs, which is what proves principal and team belong to the same org.

-- `teams` already declares UNIQUE (id, org_id) (0002_identity.sql:17-29). `agents` has only
-- PRIMARY KEY (id) and UNIQUE (org_id, name), so the composite FK target must be added.
ALTER TABLE agents ADD CONSTRAINT agents_id_org_unique UNIQUE (id, org_id);

-- Stage the old table rather than dropping it outright: the carry-forward below must read
-- from it, and an INSERT ... SELECT cannot run after a DROP.
ALTER TABLE grants RENAME TO grants_legacy;

CREATE TABLE user_grants (
    id         UUID PRIMARY KEY,
    user_id    UUID NOT NULL,
    org_id     UUID NOT NULL,
    team_id    UUID NOT NULL,
    resource   TEXT NOT NULL,
    action     TEXT NOT NULL CHECK (action IN ('read', 'create', 'update', 'delete', 'execute')),
    created_by UUID REFERENCES users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Authority is contingent on membership: removing the membership revokes the grants.
    FOREIGN KEY (user_id, org_id) REFERENCES org_memberships(user_id, org_id) ON DELETE CASCADE,
    FOREIGN KEY (team_id, org_id) REFERENCES teams(id, org_id) ON DELETE CASCADE,
    UNIQUE (user_id, team_id, resource, action)
);
CREATE INDEX idx_user_grants_user ON user_grants(user_id);
CREATE INDEX idx_user_grants_team ON user_grants(team_id);

CREATE TABLE agent_grants (
    id         UUID PRIMARY KEY,
    agent_id   UUID NOT NULL,
    org_id     UUID NOT NULL,
    team_id    UUID NOT NULL,
    resource   TEXT NOT NULL,
    action     TEXT NOT NULL CHECK (action IN ('read', 'create', 'update', 'delete', 'execute')),
    created_by UUID REFERENCES users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Deleting an agent cascades its grants; SUSPENDING one does not (identity.rs sets
    -- status, it does not delete) — that asymmetry is intended.
    FOREIGN KEY (agent_id, org_id) REFERENCES agents(id, org_id) ON DELETE CASCADE,
    FOREIGN KEY (team_id, org_id) REFERENCES teams(id, org_id) ON DELETE CASCADE,
    UNIQUE (agent_id, team_id, resource, action)
);
CREATE INDEX idx_agent_grants_agent ON agent_grants(agent_id);
CREATE INDEX idx_agent_grants_team ON agent_grants(team_id);

-- Carry forward exactly the rows the new schema considers legal. The EXISTS predicates are
-- the new foreign keys expressed as a filter, so every surviving row is insertable by
-- construction and every dropped row is precisely an orphan this feature exists to make
-- unrepresentable. Nothing legitimate is silently lost, and nothing illegal is smuggled in.
--
-- The old UNIQUE key was (principal_type, principal_id, team_id, resource, action); restricted
-- to one principal_type it collapses exactly onto each new table's four-column key, so the
-- carry-forward cannot collide. `id` and `created_at` are preserved.
INSERT INTO user_grants (id, user_id, org_id, team_id, resource, action, created_by, created_at)
SELECT g.id, g.principal_id, g.org_id, g.team_id, g.resource, g.action, g.created_by, g.created_at
FROM grants_legacy g
WHERE g.principal_type = 'user'
  AND EXISTS (SELECT 1 FROM org_memberships m WHERE m.user_id = g.principal_id AND m.org_id = g.org_id)
  AND EXISTS (SELECT 1 FROM teams t WHERE t.id = g.team_id AND t.org_id = g.org_id);

INSERT INTO agent_grants (id, agent_id, org_id, team_id, resource, action, created_by, created_at)
SELECT g.id, g.principal_id, g.org_id, g.team_id, g.resource, g.action, g.created_by, g.created_at
FROM grants_legacy g
WHERE g.principal_type = 'agent'
  AND EXISTS (SELECT 1 FROM agents a WHERE a.id = g.principal_id AND a.org_id = g.org_id)
  AND EXISTS (SELECT 1 FROM teams t WHERE t.id = g.team_id AND t.org_id = g.org_id);

DROP TABLE grants_legacy;
