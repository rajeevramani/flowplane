-- 0020: S9 traffic-first discovery sessions and Flowplane-owned gateway resources.

ALTER TABLE clusters
    ADD COLUMN owner_kind TEXT NOT NULL DEFAULT 'user' CHECK (owner_kind IN ('user', 'discovery')),
    ADD COLUMN owner_id UUID;
ALTER TABLE route_configs
    ADD COLUMN owner_kind TEXT NOT NULL DEFAULT 'user' CHECK (owner_kind IN ('user', 'discovery')),
    ADD COLUMN owner_id UUID;
ALTER TABLE listeners
    ADD COLUMN owner_kind TEXT NOT NULL DEFAULT 'user' CHECK (owner_kind IN ('user', 'discovery')),
    ADD COLUMN owner_id UUID;

ALTER TABLE clusters
    ADD CONSTRAINT clusters_owner_id_required CHECK (
        (owner_kind = 'user' AND owner_id IS NULL)
        OR (owner_kind = 'discovery' AND owner_id IS NOT NULL)
    );
ALTER TABLE route_configs
    ADD CONSTRAINT route_configs_owner_id_required CHECK (
        (owner_kind = 'user' AND owner_id IS NULL)
        OR (owner_kind = 'discovery' AND owner_id IS NOT NULL)
    );
ALTER TABLE listeners
    ADD CONSTRAINT listeners_owner_id_required CHECK (
        (owner_kind = 'user' AND owner_id IS NULL)
        OR (owner_kind = 'discovery' AND owner_id IS NOT NULL)
    );

CREATE INDEX idx_clusters_owner ON clusters(team_id, owner_kind, owner_id);
CREATE INDEX idx_route_configs_owner ON route_configs(team_id, owner_kind, owner_id);
CREATE INDEX idx_listeners_owner ON listeners(team_id, owner_kind, owner_id);

CREATE TABLE discovery_sessions (
    id                      UUID PRIMARY KEY,
    team_id                 UUID NOT NULL,
    org_id                  UUID NOT NULL,
    name                    TEXT NOT NULL,
    status                  TEXT NOT NULL CHECK (status IN ('capturing', 'completed', 'cancelled', 'failed')),
    listener_port           INT NOT NULL CHECK (listener_port BETWEEN 1024 AND 65535),
    upstream_host           TEXT NOT NULL,
    upstream_port           INT NOT NULL CHECK (upstream_port BETWEEN 1 AND 65535),
    upstream_tls            BOOLEAN NOT NULL DEFAULT false,
    validated_upstream_ip   TEXT NOT NULL,
    validated_upstream_port INT NOT NULL CHECK (validated_upstream_port BETWEEN 1 AND 65535),
    cluster_name            TEXT NOT NULL,
    route_config_name       TEXT NOT NULL,
    listener_name           TEXT NOT NULL,
    target_sample_count     INT NOT NULL CHECK (target_sample_count BETWEEN 1 AND 100000),
    max_duration_seconds    INT CHECK (max_duration_seconds BETWEEN 1 AND 86400),
    max_bytes               BIGINT NOT NULL CHECK (max_bytes BETWEEN 1 AND 1073741824),
    max_distinct_paths      INT NOT NULL CHECK (max_distinct_paths BETWEEN 1 AND 10000),
    sample_count            BIGINT NOT NULL DEFAULT 0 CHECK (sample_count >= 0),
    byte_count              BIGINT NOT NULL DEFAULT 0 CHECK (byte_count >= 0),
    path_count              BIGINT NOT NULL DEFAULT 0 CHECK (path_count >= 0),
    drop_count              BIGINT NOT NULL DEFAULT 0 CHECK (drop_count >= 0),
    started_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at            TIMESTAMPTZ,
    cancelled_at            TIMESTAMPTZ,
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (team_id, name),
    UNIQUE (id, team_id),
    FOREIGN KEY (team_id, org_id) REFERENCES teams(id, org_id) ON DELETE CASCADE
);

CREATE INDEX idx_discovery_sessions_team_status ON discovery_sessions(team_id, status, created_at DESC);
