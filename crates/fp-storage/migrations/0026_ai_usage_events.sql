-- 0026: append-only AI usage events captured by the selected upstream backend.

CREATE TABLE ai_usage_events (
    id                UUID PRIMARY KEY,
    team_id           UUID NOT NULL,
    route_config_id   UUID NOT NULL,
    provider_id       UUID NOT NULL,
    backend_position  INT,
    prompt_tokens     BIGINT NOT NULL CHECK (prompt_tokens >= 0),
    completion_tokens BIGINT NOT NULL CHECK (completion_tokens >= 0),
    total_tokens      BIGINT NOT NULL CHECK (total_tokens >= 0),
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (team_id) REFERENCES teams(id) ON DELETE CASCADE
);

CREATE INDEX idx_ai_usage_events_team_created ON ai_usage_events(team_id, created_at DESC);
CREATE INDEX idx_ai_usage_events_route_config ON ai_usage_events(team_id, route_config_id, created_at DESC);
