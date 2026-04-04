CREATE TABLE agent_grants (
    id          TEXT PRIMARY KEY,
    agent_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    org_id      TEXT NOT NULL,
    team        TEXT NOT NULL,
    grant_type  TEXT NOT NULL CHECK (grant_type IN ('cp-tool', 'gateway-tool', 'route')),

    -- cp-tool grants: which resource type and action
    resource_type   TEXT,           -- 'clusters' | 'routes' | 'listeners' | 'agents' | etc.
    action          TEXT,           -- 'read' | 'write'

    -- gateway-tool + route grants: which route and allowed HTTP methods
    route_id        TEXT REFERENCES routes(id) ON DELETE CASCADE,
    allowed_methods TEXT[],         -- ['GET', 'POST'] or NULL = all methods

    created_by  TEXT NOT NULL REFERENCES users(id),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at  TIMESTAMPTZ,

    -- Integrity: cp-tool grants need resource_type + action
    CONSTRAINT cp_tool_requires_resource CHECK (
        grant_type != 'cp-tool' OR (resource_type IS NOT NULL AND action IS NOT NULL)
    ),
    -- Integrity: gateway-tool and route grants need route_id
    CONSTRAINT gateway_route_requires_route CHECK (
        grant_type = 'cp-tool' OR route_id IS NOT NULL
    )
);

-- Dedup indexes using PARTIAL UNIQUE INDEXES (not a single UNIQUE constraint).
-- PostgreSQL treats NULLs as always distinct in UNIQUE constraints, so a single
-- UNIQUE(agent_id, grant_type, resource_type, action, route_id) would not catch
-- duplicate cp-tool grants (route_id=NULL) or duplicate route grants (resource_type=NULL).

CREATE UNIQUE INDEX idx_agent_grants_cp_unique
    ON agent_grants(agent_id, resource_type, action)
    WHERE grant_type = 'cp-tool';

CREATE UNIQUE INDEX idx_agent_grants_route_unique
    ON agent_grants(agent_id, grant_type, route_id)
    WHERE grant_type IN ('gateway-tool', 'route');

CREATE INDEX idx_agent_grants_agent_id ON agent_grants(agent_id);
CREATE INDEX idx_agent_grants_route_id ON agent_grants(route_id) WHERE route_id IS NOT NULL;

-- Drop old scope-based table if it exists (no customers, clean slate)
DROP TABLE IF EXISTS route_access_grants;
