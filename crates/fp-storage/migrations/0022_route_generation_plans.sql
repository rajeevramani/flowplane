-- 0022: S9 persisted route generation dry-run/apply plans.

CREATE TABLE route_generation_plans (
    id              UUID PRIMARY KEY,
    team_id         UUID NOT NULL,
    org_id          UUID NOT NULL,
    api_definition_id UUID NOT NULL,
    spec_version_id UUID NOT NULL,
    status          TEXT NOT NULL CHECK (status IN ('dry_run', 'applied')),
    plan            JSONB NOT NULL,
    applied_at      TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, team_id),
    FOREIGN KEY (team_id, org_id) REFERENCES teams(id, org_id) ON DELETE CASCADE,
    FOREIGN KEY (spec_version_id, api_definition_id, team_id)
        REFERENCES spec_versions(id, api_definition_id, team_id) ON DELETE CASCADE
);

CREATE INDEX idx_route_generation_plans_team_created
    ON route_generation_plans(team_id, created_at DESC);
