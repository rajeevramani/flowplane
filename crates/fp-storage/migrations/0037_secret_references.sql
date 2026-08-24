-- 0037: normalized listener/cluster SDS secret references.
--
-- JSON specs remain authoritative product configuration. These rows are same-transaction,
-- schema-enforced dependency indexes: owner deletion cascades them, while secret deletion is
-- restricted until every typed owner reference is removed. Composite FKs prove same-team use.

-- A rolling deploy may leave an old writer alive while the new candidate migrates. Serialize
-- the preflight, DDL and backfill against listener/cluster/secret writes so a valid reference
-- cannot appear between validation and backfill.
LOCK TABLE listeners, clusters, secrets IN SHARE ROW EXCLUSIVE MODE;

DO $$
DECLARE
    invalid_listener_refs BIGINT;
    invalid_cluster_refs BIGINT;
BEGIN
    SELECT count(*)
      INTO invalid_listener_refs
      FROM listeners l
      CROSS JOIN LATERAL (
          VALUES
              ('tls_certificate', l.spec #>> '{tls_context,tls_certificate_sds_secret_name}', 'tls_certificate'),
              ('validation_context', l.spec #>> '{tls_context,validation_context_sds_secret_name}', 'certificate_validation_context')
      ) AS r(usage, secret_name, required_type)
      LEFT JOIN secrets s
        ON s.team_id = l.team_id
       AND s.name = r.secret_name
     WHERE r.secret_name IS NOT NULL
       AND (s.id IS NULL OR s.secret_type <> r.required_type);

    IF invalid_listener_refs > 0 THEN
        RAISE EXCEPTION USING
            ERRCODE = '23514',
            MESSAGE = format(
                'secret reference migration found %s listener SDS reference(s) with missing or wrong-type same-team secrets',
                invalid_listener_refs
            ),
            HINT = 'create or correct the referenced same-team secrets before retrying the migration';
    END IF;

    SELECT count(*)
      INTO invalid_cluster_refs
      FROM clusters c
      CROSS JOIN LATERAL (
          VALUES
              ('validation_context', c.spec #>> '{upstream_tls,validation_context_sds_secret_name}', 'certificate_validation_context')
      ) AS r(usage, secret_name, required_type)
      LEFT JOIN secrets s
        ON s.team_id = c.team_id
       AND s.name = r.secret_name
     WHERE r.secret_name IS NOT NULL
       AND (s.id IS NULL OR s.secret_type <> r.required_type);

    IF invalid_cluster_refs > 0 THEN
        RAISE EXCEPTION USING
            ERRCODE = '23514',
            MESSAGE = format(
                'secret reference migration found %s cluster SDS reference(s) with missing or wrong-type same-team secrets',
                invalid_cluster_refs
            ),
            HINT = 'create or correct the referenced same-team secrets before retrying the migration';
    END IF;
END $$;

CREATE TABLE listener_secret_refs (
    listener_id UUID NOT NULL,
    team_id     UUID NOT NULL,
    secret_id   UUID NOT NULL,
    usage       TEXT NOT NULL CHECK (usage IN ('tls_certificate', 'validation_context')),
    PRIMARY KEY (listener_id, usage),
    FOREIGN KEY (listener_id, team_id) REFERENCES listeners(id, team_id) ON DELETE CASCADE,
    FOREIGN KEY (secret_id, team_id) REFERENCES secrets(id, team_id) ON DELETE RESTRICT
);
CREATE INDEX idx_listener_secret_refs_secret ON listener_secret_refs(secret_id, team_id);

CREATE TABLE cluster_secret_refs (
    cluster_id UUID NOT NULL,
    team_id    UUID NOT NULL,
    secret_id  UUID NOT NULL,
    usage      TEXT NOT NULL CHECK (usage = 'validation_context'),
    PRIMARY KEY (cluster_id, usage),
    FOREIGN KEY (cluster_id, team_id) REFERENCES clusters(id, team_id) ON DELETE CASCADE,
    FOREIGN KEY (secret_id, team_id) REFERENCES secrets(id, team_id) ON DELETE RESTRICT
);
CREATE INDEX idx_cluster_secret_refs_secret ON cluster_secret_refs(secret_id, team_id);

INSERT INTO listener_secret_refs (listener_id, team_id, secret_id, usage)
SELECT l.id, l.team_id, s.id, r.usage
  FROM listeners l
  CROSS JOIN LATERAL (
      VALUES
          ('tls_certificate', l.spec #>> '{tls_context,tls_certificate_sds_secret_name}'),
          ('validation_context', l.spec #>> '{tls_context,validation_context_sds_secret_name}')
  ) AS r(usage, secret_name)
  JOIN secrets s
    ON s.team_id = l.team_id
   AND s.name = r.secret_name
 WHERE r.secret_name IS NOT NULL;

INSERT INTO cluster_secret_refs (cluster_id, team_id, secret_id, usage)
SELECT c.id, c.team_id, s.id, 'validation_context'
  FROM clusters c
  JOIN secrets s
    ON s.team_id = c.team_id
   AND s.name = c.spec #>> '{upstream_tls,validation_context_sds_secret_name}'
 WHERE c.spec #>> '{upstream_tls,validation_context_sds_secret_name}' IS NOT NULL;
