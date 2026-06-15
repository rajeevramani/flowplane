-- 0024: S10 AI route resources and backend/provider references.

CREATE TABLE ai_routes (
    id                UUID PRIMARY KEY,
    team_id           UUID NOT NULL,
    org_id            UUID NOT NULL,
    name              TEXT NOT NULL,
    spec              JSONB NOT NULL,
    status            TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'stale')),
    cluster_names     TEXT[] NOT NULL,
    route_config_name TEXT NOT NULL,
    listener_name     TEXT NOT NULL,
    version           BIGINT NOT NULL DEFAULT 1,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (team_id, name),
    UNIQUE (id, team_id),
    FOREIGN KEY (team_id, org_id) REFERENCES teams(id, org_id) ON DELETE CASCADE
);

CREATE TABLE ai_route_backends (
    ai_route_id UUID NOT NULL,
    team_id     UUID NOT NULL,
    provider_id UUID NOT NULL,
    position    INT NOT NULL,
    PRIMARY KEY (ai_route_id, position),
    FOREIGN KEY (ai_route_id, team_id) REFERENCES ai_routes(id, team_id) ON DELETE CASCADE,
    FOREIGN KEY (provider_id, team_id) REFERENCES ai_providers(id, team_id) ON DELETE RESTRICT
);

CREATE INDEX idx_ai_routes_team ON ai_routes(team_id);
CREATE INDEX idx_ai_route_backends_provider ON ai_route_backends(team_id, provider_id);
