-- 0031: AI gateway end-to-end trace rows (feature ai-gateway-e2e-trace, slice s2).
--
-- One observational row per AI data-plane request, keyed (team_id, request_id) — the
-- server-owned x-request-id pinned by slice s1. Hop detail lives only in the `hops`
-- JSONB array (sole store, no relational projection). `expires_at` is stamped at
-- insert from the team's ai_retention_policies row (30-day default when absent), so
-- every row is retention-correct from day one; the sweep task arrives in slice s5.
--
-- No prompt/completion/body columns exist by construction (design AC 6): the schema
-- cannot carry request or response payloads, and the credential hop records the auth
-- header *name* plus an outcome enum only.

CREATE TABLE ai_retention_policies (
    id             UUID PRIMARY KEY,
    team_id        UUID NOT NULL,
    org_id         UUID NOT NULL,
    trace_ttl_days INT NOT NULL CHECK (trace_ttl_days > 0),
    version        BIGINT NOT NULL DEFAULT 1,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (team_id),
    FOREIGN KEY (team_id, org_id) REFERENCES teams(id, org_id) ON DELETE CASCADE
);

CREATE TABLE ai_trace_events (
    id              UUID PRIMARY KEY,
    team_id         UUID NOT NULL,
    request_id      TEXT NOT NULL,
    trace_id        TEXT,
    route_config_id UUID NOT NULL,
    listener_id     UUID,
    provider_id     UUID,
    model           TEXT,
    status_code     INT,
    failure_hop     TEXT,
    hops            JSONB NOT NULL DEFAULT '[]'::jsonb,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at      TIMESTAMPTZ NOT NULL,
    FOREIGN KEY (team_id) REFERENCES teams(id) ON DELETE CASCADE
);

-- The upsert key: both ExtProc streams of one HTTP request merge into one row.
CREATE UNIQUE INDEX uq_ai_trace_events_team_request ON ai_trace_events(team_id, request_id);
CREATE INDEX idx_ai_trace_events_team_created ON ai_trace_events(team_id, created_at DESC);
CREATE INDEX idx_ai_trace_events_expires ON ai_trace_events(expires_at);
