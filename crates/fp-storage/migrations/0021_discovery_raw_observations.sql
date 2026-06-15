-- 0021: S9 discovery provenance for raw observations.

ALTER TABLE raw_observations
    ALTER COLUMN capture_session_id DROP NOT NULL;

ALTER TABLE raw_observations
    DROP CONSTRAINT raw_observations_team_id_capture_session_id_request_id_key;

CREATE UNIQUE INDEX idx_raw_observations_capture_request
    ON raw_observations(team_id, capture_session_id, request_id)
    WHERE capture_session_id IS NOT NULL;

CREATE TABLE discovery_raw_observations (
    raw_observation_id       UUID PRIMARY KEY,
    team_id                  UUID NOT NULL,
    request_id               TEXT NOT NULL,
    discovery_session_id     UUID NOT NULL,
    discovery_listener_id    UUID NOT NULL,
    observed_host            TEXT NOT NULL,
    observed_sni             TEXT,
    route_matched            BOOLEAN NOT NULL DEFAULT false,
    forwarded_upstream_host  TEXT NOT NULL,
    forwarded_upstream_port  INT NOT NULL CHECK (forwarded_upstream_port BETWEEN 1 AND 65535),
    forwarded_upstream_ip    TEXT NOT NULL,
    forwarded_upstream_tls   BOOLEAN NOT NULL DEFAULT false,
    created_at               TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (team_id, discovery_session_id, request_id),
    FOREIGN KEY (raw_observation_id, team_id) REFERENCES raw_observations(id, team_id)
        ON DELETE CASCADE,
    FOREIGN KEY (discovery_session_id, team_id) REFERENCES discovery_sessions(id, team_id)
        ON DELETE CASCADE
);

CREATE INDEX idx_discovery_raw_observations_session
    ON discovery_raw_observations(team_id, discovery_session_id, observed_host, observed_sni);
