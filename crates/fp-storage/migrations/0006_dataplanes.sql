-- 0006: dataplanes + proxy certificate registry (S5.4, spec/04 §1.3).
-- The registry row — looked up by the globally-unique SPIFFE URI from the client cert —
-- is the ONLY authorization source for an xDS connection; SAN team segments and node ids
-- are never trusted. Timestamps are TIMESTAMPTZ (fixes v1's ISO-8601-in-TEXT smell,
-- spec/03 §1.2). Composite FKs prove same-team relationships at the schema level.

CREATE TABLE dataplanes (
    id          UUID PRIMARY KEY,
    team_id     UUID NOT NULL,
    org_id      UUID NOT NULL,
    name        TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    version     BIGINT NOT NULL DEFAULT 1,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (team_id, name),
    UNIQUE (id, team_id),
    FOREIGN KEY (team_id, org_id) REFERENCES teams(id, org_id) ON DELETE CASCADE
);
CREATE INDEX idx_dataplanes_team ON dataplanes(team_id);

CREATE TABLE proxy_certificates (
    id                UUID PRIMARY KEY,
    team_id           UUID NOT NULL,
    dataplane_id      UUID NOT NULL,
    spiffe_uri        TEXT NOT NULL UNIQUE,   -- global binding key for mTLS lookup
    serial_number     TEXT NOT NULL,
    issued_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at        TIMESTAMPTZ NOT NULL,
    issued_by_user_id UUID REFERENCES users(id) ON DELETE SET NULL,
    revoked_at        TIMESTAMPTZ,            -- soft revocation; the row is the audit record
    revoked_reason    TEXT,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (team_id, serial_number),
    -- RESTRICT: a dataplane with live certificate records cannot be silently deleted.
    FOREIGN KEY (dataplane_id, team_id) REFERENCES dataplanes(id, team_id) ON DELETE RESTRICT
);
CREATE INDEX idx_proxy_certs_team ON proxy_certificates(team_id);
CREATE INDEX idx_proxy_certs_dataplane ON proxy_certificates(dataplane_id);
