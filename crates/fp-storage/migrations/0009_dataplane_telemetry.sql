-- 0009: dataplane liveness + coarse telemetry counters for S6.5.

ALTER TABLE dataplanes
    ADD COLUMN last_heartbeat_at TIMESTAMPTZ,
    ADD COLUMN last_config_verify_at TIMESTAMPTZ,
    ADD COLUMN total_requests BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN total_errors BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN warming_failures BIGINT NOT NULL DEFAULT 0;

CREATE INDEX idx_dataplanes_last_heartbeat ON dataplanes(team_id, last_heartbeat_at);
