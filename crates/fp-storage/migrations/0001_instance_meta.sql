-- 0001: instance metadata key/value store.
-- Used for instance-level facts (schema provenance, install id). Tenancy tables land in S2.
CREATE TABLE instance_meta (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

INSERT INTO instance_meta (key, value) VALUES ('schema_origin', 'flowplane');
