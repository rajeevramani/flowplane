-- Unified grants table replacing agent_grants + user_team_memberships.scopes
-- Part A: Create grants table

CREATE TABLE IF NOT EXISTS grants (
    id TEXT PRIMARY KEY,
    principal_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    org_id TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    team_id TEXT NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
    grant_type TEXT NOT NULL CHECK (grant_type IN ('resource', 'gateway-tool', 'route')),
    resource_type TEXT,
    action TEXT,
    route_id TEXT REFERENCES routes(id) ON DELETE CASCADE,
    allowed_methods TEXT[],
    created_by TEXT NOT NULL REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ,

    CONSTRAINT resource_requires_type_action CHECK (
        grant_type != 'resource' OR (resource_type IS NOT NULL AND action IS NOT NULL)
    ),
    CONSTRAINT gateway_route_requires_route CHECK (
        grant_type = 'resource' OR route_id IS NOT NULL
    )
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_grants_resource_unique
    ON grants(principal_id, team_id, resource_type, action)
    WHERE grant_type = 'resource';

CREATE UNIQUE INDEX IF NOT EXISTS idx_grants_route_unique
    ON grants(principal_id, grant_type, route_id)
    WHERE grant_type IN ('gateway-tool', 'route');

CREATE INDEX IF NOT EXISTS idx_grants_principal_id ON grants(principal_id);
CREATE INDEX IF NOT EXISTS idx_grants_org_id ON grants(org_id);
CREATE INDEX IF NOT EXISTS idx_grants_team_id ON grants(team_id);

-- Part B: Migrate agent_grants data (cp-tool -> resource, keep gateway-tool/route)

INSERT INTO grants (id, principal_id, org_id, team_id, grant_type, resource_type, action, route_id, allowed_methods, created_by, created_at, expires_at)
SELECT
    ag.id,
    ag.agent_id,
    ag.org_id,
    t.id,
    CASE WHEN ag.grant_type = 'cp-tool' THEN 'resource' ELSE ag.grant_type END,
    ag.resource_type,
    ag.action,
    ag.route_id,
    ag.allowed_methods,
    ag.created_by,
    ag.created_at,
    ag.expires_at
FROM agent_grants ag
JOIN teams t ON t.name = ag.team AND t.org_id = ag.org_id
ON CONFLICT DO NOTHING;

-- Part C: Migrate user_team_memberships.scopes — team:X:resource:action prefixed scopes

INSERT INTO grants (id, principal_id, org_id, team_id, grant_type, resource_type, action, created_by, created_at)
SELECT
    'migrated-' || md5(utm.user_id || ':' || t.id || ':' || parts[3] || ':' || parts[4]),
    utm.user_id,
    t.org_id,
    t.id,
    'resource',
    parts[3],
    parts[4],
    utm.user_id,
    NOW()
FROM user_team_memberships utm
JOIN teams t ON t.id = utm.team
CROSS JOIN LATERAL (
    SELECT string_to_array(scope_val, ':') AS parts, scope_val
    FROM jsonb_array_elements_text(
        CASE
            WHEN utm.scopes IS NOT NULL AND utm.scopes::text != 'null' AND utm.scopes::text != ''
            THEN utm.scopes::jsonb
            ELSE '[]'::jsonb
        END
    ) AS scope_val
    WHERE scope_val LIKE 'team:%:%:%'
      AND scope_val NOT LIKE '%:*'
) sub
WHERE array_length(parts, 1) = 4
  AND parts[4] != '*'
  AND parts[3] != 'admin'
ON CONFLICT DO NOTHING;

-- Part C continued: bare resource:action scopes (no team: prefix)

INSERT INTO grants (id, principal_id, org_id, team_id, grant_type, resource_type, action, created_by, created_at)
SELECT
    'migrated-bare-' || md5(utm.user_id || ':' || t.id || ':' || parts[1] || ':' || parts[2]),
    utm.user_id,
    t.org_id,
    t.id,
    'resource',
    parts[1],
    parts[2],
    utm.user_id,
    NOW()
FROM user_team_memberships utm
JOIN teams t ON t.id = utm.team
CROSS JOIN LATERAL (
    SELECT string_to_array(scope_val, ':') AS parts, scope_val
    FROM jsonb_array_elements_text(
        CASE
            WHEN utm.scopes IS NOT NULL AND utm.scopes::text != 'null' AND utm.scopes::text != ''
            THEN utm.scopes::jsonb
            ELSE '[]'::jsonb
        END
    ) AS scope_val
    WHERE scope_val NOT LIKE 'team:%'
      AND scope_val NOT LIKE 'org:%'
      AND scope_val NOT LIKE 'admin:%'
      AND scope_val LIKE '%:%'
      AND scope_val NOT LIKE '%:*'
) sub
WHERE array_length(parts, 1) = 2
  AND parts[2] != '*'
ON CONFLICT DO NOTHING;

-- Part D: Add user_type CHECK constraint (HIGH-10)

ALTER TABLE users ADD CONSTRAINT chk_user_type CHECK (user_type IN ('human', 'machine'));

-- Part E: Drop old tables

DROP TABLE IF EXISTS agent_grants;
DROP TABLE IF EXISTS scopes;
