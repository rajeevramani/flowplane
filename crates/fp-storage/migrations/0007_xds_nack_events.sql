-- 0007: persisted dataplane NACKs (S5.5, spec/10 §5). Telemetry-grade rows: written by
-- the xDS stream handler (not the service/outbox path — a NACK is an observation, not a
-- mutation), surfaced per team through the status API and, later, warming reports (S6).

CREATE TABLE xds_nack_events (
    id                    UUID PRIMARY KEY,
    team_id               UUID NOT NULL,
    org_id                UUID NOT NULL,
    node_id               TEXT NOT NULL DEFAULT '',
    type_url              TEXT NOT NULL,
    version_rejected      TEXT NOT NULL DEFAULT '',
    error_message         TEXT NOT NULL,
    quarantined_resources JSONB NOT NULL DEFAULT '[]',
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (team_id, org_id) REFERENCES teams(id, org_id) ON DELETE CASCADE
);
CREATE INDEX idx_xds_nack_team_time ON xds_nack_events(team_id, created_at DESC);
