-- 0012: S8.5 durable raw observations for learning capture.
-- One row represents the accepted, sanitized view of a request observed during a capture
-- session. Metadata and body events for the same request merge by request_id.

CREATE TABLE raw_observations (
    id                    UUID PRIMARY KEY,
    team_id               UUID NOT NULL,
    org_id                UUID NOT NULL,
    capture_session_id    UUID NOT NULL,
    request_id            TEXT NOT NULL,
    method                TEXT NOT NULL,
    path                  TEXT NOT NULL,
    response_status       INT CHECK (response_status BETWEEN 100 AND 599),
    request_headers       JSONB NOT NULL DEFAULT '{}'::jsonb,
    response_headers      JSONB NOT NULL DEFAULT '{}'::jsonb,
    request_body          TEXT,
    response_body         TEXT,
    request_body_truncated BOOLEAN NOT NULL DEFAULT false,
    response_body_truncated BOOLEAN NOT NULL DEFAULT false,
    request_body_bytes    BIGINT NOT NULL DEFAULT 0 CHECK (request_body_bytes >= 0),
    response_body_bytes   BIGINT NOT NULL DEFAULT 0 CHECK (response_body_bytes >= 0),
    metadata_seen         BOOLEAN NOT NULL DEFAULT false,
    body_seen             BOOLEAN NOT NULL DEFAULT false,
    observed_at           TIMESTAMPTZ NOT NULL,
    expires_at            TIMESTAMPTZ NOT NULL,
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (team_id, capture_session_id, request_id),
    UNIQUE (id, team_id),
    FOREIGN KEY (team_id, org_id) REFERENCES teams(id, org_id) ON DELETE CASCADE,
    FOREIGN KEY (capture_session_id, team_id) REFERENCES capture_sessions(id, team_id)
        ON DELETE CASCADE
);

CREATE INDEX idx_raw_observations_session ON raw_observations(capture_session_id, created_at);
CREATE INDEX idx_raw_observations_expires_at ON raw_observations(expires_at);
CREATE INDEX idx_raw_observations_team_path ON raw_observations(team_id, capture_session_id, path);
