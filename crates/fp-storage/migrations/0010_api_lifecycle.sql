-- 0010: S8/D-017 API lifecycle foundation.
-- API definitions are team-owned config roots. Imported specs, learned specs, generated
-- tools, route bindings, and retention policies all hang from this spine with composite
-- (id, team_id) FKs so cross-team joins are unrepresentable.

CREATE TABLE api_definitions (
    id           UUID PRIMARY KEY,
    team_id      UUID NOT NULL,
    org_id       UUID NOT NULL,
    name         TEXT NOT NULL,
    display_name TEXT NOT NULL DEFAULT '',
    description  TEXT NOT NULL DEFAULT '',
    version      BIGINT NOT NULL DEFAULT 1,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (team_id, name),
    UNIQUE (id, team_id),
    FOREIGN KEY (team_id, org_id) REFERENCES teams(id, org_id) ON DELETE CASCADE
);
CREATE INDEX idx_api_definitions_team ON api_definitions(team_id);

CREATE TABLE api_route_bindings (
    id                UUID PRIMARY KEY,
    team_id           UUID NOT NULL,
    org_id            UUID NOT NULL,
    api_definition_id UUID NOT NULL,
    route_config_id   UUID NOT NULL,
    listener_id       UUID,
    name              TEXT NOT NULL,
    virtual_host      TEXT,
    route             TEXT,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (team_id, name),
    UNIQUE (api_definition_id, route_config_id, virtual_host, route),
    UNIQUE (id, team_id),
    FOREIGN KEY (team_id, org_id) REFERENCES teams(id, org_id) ON DELETE CASCADE,
    FOREIGN KEY (api_definition_id, team_id) REFERENCES api_definitions(id, team_id) ON DELETE CASCADE,
    FOREIGN KEY (route_config_id, team_id) REFERENCES route_configs(id, team_id) ON DELETE RESTRICT,
    FOREIGN KEY (listener_id, team_id) REFERENCES listeners(id, team_id) ON DELETE RESTRICT
);
CREATE INDEX idx_api_route_bindings_api ON api_route_bindings(api_definition_id);
CREATE INDEX idx_api_route_bindings_route_config ON api_route_bindings(route_config_id);

CREATE TABLE spec_versions (
    id                UUID PRIMARY KEY,
    team_id           UUID NOT NULL,
    org_id            UUID NOT NULL,
    api_definition_id UUID NOT NULL,
    version           BIGINT NOT NULL,
    source_kind       TEXT NOT NULL CHECK (source_kind IN ('imported', 'learned', 'manual')),
    format            TEXT NOT NULL CHECK (format IN ('openapi3')),
    spec              JSONB NOT NULL,
    spec_hash         TEXT NOT NULL,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (api_definition_id, version),
    UNIQUE (api_definition_id, spec_hash),
    UNIQUE (id, team_id),
    FOREIGN KEY (team_id, org_id) REFERENCES teams(id, org_id) ON DELETE CASCADE,
    FOREIGN KEY (api_definition_id, team_id) REFERENCES api_definitions(id, team_id) ON DELETE CASCADE
);
CREATE INDEX idx_spec_versions_api ON spec_versions(api_definition_id, version DESC);

CREATE FUNCTION forbid_spec_version_update() RETURNS trigger AS $$
BEGIN
    RAISE EXCEPTION 'spec_versions are immutable; insert a new version instead'
        USING ERRCODE = '45000';
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_spec_versions_no_update
BEFORE UPDATE ON spec_versions
FOR EACH ROW EXECUTE FUNCTION forbid_spec_version_update();

CREATE TABLE api_tools (
    id                UUID PRIMARY KEY,
    team_id           UUID NOT NULL,
    org_id            UUID NOT NULL,
    api_definition_id UUID NOT NULL,
    spec_version_id   UUID NOT NULL,
    name              TEXT NOT NULL,
    operation_id      TEXT NOT NULL,
    method            TEXT NOT NULL CHECK (method IN ('GET', 'POST', 'PUT', 'PATCH', 'DELETE', 'OPTIONS', 'HEAD')),
    path              TEXT NOT NULL,
    input_schema      JSONB NOT NULL DEFAULT '{}'::jsonb,
    output_schema     JSONB NOT NULL DEFAULT '{}'::jsonb,
    enabled           BOOLEAN NOT NULL DEFAULT true,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (team_id, name),
    UNIQUE (spec_version_id, operation_id),
    UNIQUE (id, team_id),
    FOREIGN KEY (team_id, org_id) REFERENCES teams(id, org_id) ON DELETE CASCADE,
    FOREIGN KEY (api_definition_id, team_id) REFERENCES api_definitions(id, team_id) ON DELETE CASCADE,
    FOREIGN KEY (spec_version_id, team_id) REFERENCES spec_versions(id, team_id) ON DELETE CASCADE
);
CREATE INDEX idx_api_tools_api ON api_tools(api_definition_id);
CREATE INDEX idx_api_tools_spec_version ON api_tools(spec_version_id);

CREATE TABLE api_retention_policies (
    id                       UUID PRIMARY KEY,
    team_id                  UUID NOT NULL,
    org_id                   UUID NOT NULL,
    api_definition_id         UUID,
    name                     TEXT NOT NULL,
    raw_observation_ttl_days  INT NOT NULL CHECK (raw_observation_ttl_days BETWEEN 1 AND 365),
    max_spec_versions         INT NOT NULL CHECK (max_spec_versions BETWEEN 1 AND 500),
    created_at               TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at               TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (team_id, name),
    UNIQUE (api_definition_id),
    UNIQUE (id, team_id),
    FOREIGN KEY (team_id, org_id) REFERENCES teams(id, org_id) ON DELETE CASCADE,
    FOREIGN KEY (api_definition_id, team_id) REFERENCES api_definitions(id, team_id) ON DELETE CASCADE
);
CREATE INDEX idx_api_retention_policies_team ON api_retention_policies(team_id);
