-- Exposure identity/provenance only: gateway specs remain the configuration authority.
-- Deliberately no legacy backfill: matching names cannot establish deletion ownership.
CREATE TABLE exposures (
    id UUID PRIMARY KEY,
    team_id UUID NOT NULL,
    org_id UUID NOT NULL,
    name TEXT NOT NULL,
    cluster_id UUID NOT NULL,
    route_config_id UUID NOT NULL,
    listener_id UUID NOT NULL,
    virtual_host TEXT NOT NULL,
    route_name TEXT NOT NULL,
    cleanup_listener BOOLEAN NOT NULL,
    cleanup_route_config BOOLEAN NOT NULL,
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (team_id, name),
    UNIQUE (id, team_id),
    UNIQUE (cluster_id, team_id),
    UNIQUE (route_config_id, virtual_host, route_name),
    FOREIGN KEY (team_id, org_id) REFERENCES teams(id, org_id) ON DELETE CASCADE,
    CONSTRAINT exposures_cluster_fk FOREIGN KEY (cluster_id, team_id)
        REFERENCES clusters(id, team_id) ON DELETE RESTRICT,
    CONSTRAINT exposures_route_config_fk FOREIGN KEY (route_config_id, team_id)
        REFERENCES route_configs(id, team_id) ON DELETE RESTRICT,
    CONSTRAINT exposures_listener_fk FOREIGN KEY (listener_id, team_id)
        REFERENCES listeners(id, team_id) ON DELETE RESTRICT
);
CREATE INDEX idx_exposures_route_config ON exposures(route_config_id, team_id);
CREATE INDEX idx_exposures_listener ON exposures(listener_id, team_id);
