-- 0011: S8.3 bounded capture sessions for the learning loop.
-- Sessions are team-owned and target either an ApiDefinition or an explicit route scope.
-- Composite FKs keep cross-team capture references unrepresentable.

CREATE TABLE capture_sessions (
    id                  UUID PRIMARY KEY,
    team_id             UUID NOT NULL,
    org_id              UUID NOT NULL,
    name                TEXT NOT NULL,
    status              TEXT NOT NULL CHECK (status IN ('capturing', 'completed', 'cancelled', 'failed')),
    api_definition_id   UUID,
    route_config_id     UUID,
    listener_id         UUID,
    virtual_host        TEXT,
    route               TEXT,
    target_sample_count INT NOT NULL CHECK (target_sample_count BETWEEN 1 AND 100000),
    max_duration_seconds INT CHECK (max_duration_seconds BETWEEN 1 AND 86400),
    max_bytes           BIGINT NOT NULL CHECK (max_bytes BETWEEN 1 AND 1073741824),
    max_distinct_paths  INT NOT NULL CHECK (max_distinct_paths BETWEEN 1 AND 10000),
    sample_count        BIGINT NOT NULL DEFAULT 0 CHECK (sample_count >= 0),
    byte_count          BIGINT NOT NULL DEFAULT 0 CHECK (byte_count >= 0),
    path_count          BIGINT NOT NULL DEFAULT 0 CHECK (path_count >= 0),
    drop_count          BIGINT NOT NULL DEFAULT 0 CHECK (drop_count >= 0),
    started_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at        TIMESTAMPTZ,
    cancelled_at        TIMESTAMPTZ,
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (team_id, name),
    UNIQUE (id, team_id),
    CHECK (
        (api_definition_id IS NOT NULL AND route_config_id IS NULL AND listener_id IS NULL
            AND virtual_host IS NULL AND route IS NULL)
        OR (api_definition_id IS NULL AND route_config_id IS NOT NULL)
    ),
    FOREIGN KEY (team_id, org_id) REFERENCES teams(id, org_id) ON DELETE CASCADE,
    FOREIGN KEY (api_definition_id, team_id) REFERENCES api_definitions(id, team_id) ON DELETE CASCADE,
    FOREIGN KEY (route_config_id, team_id) REFERENCES route_configs(id, team_id) ON DELETE RESTRICT,
    FOREIGN KEY (listener_id, team_id) REFERENCES listeners(id, team_id) ON DELETE RESTRICT
);

CREATE INDEX idx_capture_sessions_team_status ON capture_sessions(team_id, status, created_at DESC);
CREATE INDEX idx_capture_sessions_api ON capture_sessions(api_definition_id);
CREATE INDEX idx_capture_sessions_route_config ON capture_sessions(route_config_id);
