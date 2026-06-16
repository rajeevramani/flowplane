-- 0027: S10 AI budgets and authoritative fixed-window counters.

CREATE TABLE ai_budgets (
    id                      UUID PRIMARY KEY,
    team_id                 UUID NOT NULL,
    org_id                  UUID NOT NULL,
    name                    TEXT NOT NULL,
    mode                    TEXT NOT NULL CHECK (mode IN ('shadow', 'enforcing')),
    limit_units             BIGINT NOT NULL CHECK (limit_units > 0),
    window_seconds          INT NOT NULL CHECK (window_seconds > 0),
    provider_id             UUID,
    route_config_id         UUID,
    prompt_token_weight     INT NOT NULL CHECK (prompt_token_weight >= 0),
    completion_token_weight INT NOT NULL CHECK (completion_token_weight >= 0),
    version                 BIGINT NOT NULL DEFAULT 1,
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (team_id, name),
    UNIQUE (id, team_id),
    CHECK (prompt_token_weight > 0 OR completion_token_weight > 0),
    FOREIGN KEY (team_id, org_id) REFERENCES teams(id, org_id) ON DELETE CASCADE,
    FOREIGN KEY (provider_id, team_id) REFERENCES ai_providers(id, team_id) ON DELETE RESTRICT,
    FOREIGN KEY (route_config_id, team_id) REFERENCES route_configs(id, team_id) ON DELETE RESTRICT
);

CREATE TABLE ai_budget_counters (
    budget_id    UUID NOT NULL,
    team_id      UUID NOT NULL,
    window_start TIMESTAMPTZ NOT NULL,
    used_units   BIGINT NOT NULL CHECK (used_units >= 0),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (budget_id, window_start),
    FOREIGN KEY (budget_id, team_id) REFERENCES ai_budgets(id, team_id) ON DELETE CASCADE
);

CREATE INDEX idx_ai_budgets_team ON ai_budgets(team_id);
CREATE INDEX idx_ai_budget_counters_team_window ON ai_budget_counters(team_id, window_start DESC);
