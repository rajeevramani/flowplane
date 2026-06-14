-- 0013: one team-default API retention policy per team.
--
-- PostgreSQL UNIQUE constraints allow multiple NULL values, so 0010's
-- UNIQUE(api_definition_id) only protected API-specific policies. Keep the newest
-- legacy default per team, then enforce the intended default policy cardinality.
WITH ranked_defaults AS (
    SELECT
        id,
        row_number() OVER (PARTITION BY team_id ORDER BY created_at DESC, id DESC) AS rank
    FROM api_retention_policies
    WHERE api_definition_id IS NULL
)
DELETE FROM api_retention_policies policy
USING ranked_defaults ranked
WHERE policy.id = ranked.id
  AND ranked.rank > 1;

CREATE UNIQUE INDEX idx_api_retention_policies_one_team_default
    ON api_retention_policies(team_id)
    WHERE api_definition_id IS NULL;
