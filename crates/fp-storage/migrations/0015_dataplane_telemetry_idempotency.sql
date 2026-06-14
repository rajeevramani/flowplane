-- 0015: idempotency ledger for dataplane telemetry deltas.
--
-- Telemetry delivery is at-least-once. Persist accepted report keys so retries of the
-- same delta return the current dataplane state without incrementing counters again.
CREATE TABLE dataplane_telemetry_reports (
    id                UUID PRIMARY KEY,
    team_id           UUID NOT NULL,
    dataplane_id      UUID NOT NULL,
    idempotency_key   TEXT NOT NULL CHECK (length(idempotency_key) BETWEEN 1 AND 200),
    requests_delta    BIGINT NOT NULL CHECK (requests_delta >= 0),
    errors_delta      BIGINT NOT NULL CHECK (errors_delta >= 0),
    warming_failures_delta BIGINT NOT NULL CHECK (warming_failures_delta >= 0),
    config_verified   BOOLEAN NOT NULL,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (team_id, dataplane_id, idempotency_key),
    FOREIGN KEY (dataplane_id, team_id) REFERENCES dataplanes(id, team_id) ON DELETE CASCADE
);

CREATE INDEX idx_dataplane_telemetry_reports_dataplane
    ON dataplane_telemetry_reports(team_id, dataplane_id, created_at);
