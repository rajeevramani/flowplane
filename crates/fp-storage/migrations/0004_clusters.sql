-- 0004: clusters (first gateway resource; the vertical pattern for all that follow).
-- Single representation: the validated spec lives in ONE jsonb column — no projection
-- tables to drift out of sync (v1's dual-write bug class, spec/03 §8.5). Per-team name
-- uniqueness (no cross-tenant name oracle); composite FK proves team ∈ org; (id, team_id)
-- unique target lets later tables prove same-team references at the schema level.

CREATE TABLE clusters (
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
CREATE INDEX idx_clusters_team ON clusters(team_id);
