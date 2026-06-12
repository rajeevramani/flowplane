-- 0005: listeners, route_configs, and NORMALIZED cluster references.
-- route_config_cluster_refs replaces v1's FK-by-name + ON DELETE CASCADE (which silently
-- deleted route trees, spec/03 §8.6): references are real id-based rows maintained in the
-- same transaction as the spec, and cluster deletion is RESTRICTED with an actionable error.
-- The (team_id, id) composite FKs prove every reference stays inside one team.

CREATE TABLE route_configs (
    id         UUID PRIMARY KEY,
    team_id    UUID NOT NULL,
    org_id     UUID NOT NULL,
    name       TEXT NOT NULL,
    spec       JSONB NOT NULL,
    version    BIGINT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (team_id, name),
    UNIQUE (id, team_id),
    FOREIGN KEY (team_id, org_id) REFERENCES teams(id, org_id) ON DELETE CASCADE
);
CREATE INDEX idx_route_configs_team ON route_configs(team_id);

CREATE TABLE listeners (
    id         UUID PRIMARY KEY,
    team_id    UUID NOT NULL,
    org_id     UUID NOT NULL,
    name       TEXT NOT NULL,
    spec       JSONB NOT NULL,
    version    BIGINT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (team_id, name),
    UNIQUE (id, team_id),
    FOREIGN KEY (team_id, org_id) REFERENCES teams(id, org_id) ON DELETE CASCADE
);
CREATE INDEX idx_listeners_team ON listeners(team_id);
-- Port allocation across dataplanes is resolved at xDS binding time (S5); per-team the
-- same port cannot be bound twice.
CREATE UNIQUE INDEX idx_listeners_team_port ON listeners(team_id, ((spec->>'port')::int));

-- Route-action cluster references. NO CASCADE from clusters: deleting a referenced cluster
-- fails with the dependent list. CASCADE from route_configs: the refs are part of the spec.
CREATE TABLE route_config_cluster_refs (
    route_config_id UUID NOT NULL,
    cluster_id      UUID NOT NULL,
    team_id         UUID NOT NULL,
    PRIMARY KEY (route_config_id, cluster_id),
    FOREIGN KEY (route_config_id, team_id) REFERENCES route_configs(id, team_id) ON DELETE CASCADE,
    FOREIGN KEY (cluster_id, team_id) REFERENCES clusters(id, team_id) ON DELETE RESTRICT
);
CREATE INDEX idx_rc_cluster_refs_cluster ON route_config_cluster_refs(cluster_id);

-- Listener -> route_config references (same discipline).
CREATE TABLE listener_route_config_refs (
    listener_id     UUID NOT NULL,
    route_config_id UUID NOT NULL,
    team_id         UUID NOT NULL,
    PRIMARY KEY (listener_id, route_config_id),
    FOREIGN KEY (listener_id, team_id) REFERENCES listeners(id, team_id) ON DELETE CASCADE,
    FOREIGN KEY (route_config_id, team_id) REFERENCES route_configs(id, team_id) ON DELETE RESTRICT
);
CREATE INDEX idx_listener_rc_refs_rc ON listener_route_config_refs(route_config_id);
