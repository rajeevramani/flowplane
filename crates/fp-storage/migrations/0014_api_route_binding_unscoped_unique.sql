-- 0014: one unscoped API route binding per API/route-config pair.
--
-- PostgreSQL UNIQUE constraints allow multiple NULL values, so 0010's
-- UNIQUE(api_definition_id, route_config_id, virtual_host, route) did not protect
-- whole-route-config bindings where both selectors are NULL. Keep the newest legacy
-- unscoped binding per team/API/route-config, then enforce that cardinality.
WITH ranked_unscoped AS (
    SELECT
        id,
        row_number() OVER (
            PARTITION BY team_id, api_definition_id, route_config_id
            ORDER BY created_at DESC, id DESC
        ) AS rank
    FROM api_route_bindings
    WHERE virtual_host IS NULL
      AND route IS NULL
)
DELETE FROM api_route_bindings binding
USING ranked_unscoped ranked
WHERE binding.id = ranked.id
  AND ranked.rank > 1;

CREATE UNIQUE INDEX idx_api_route_bindings_one_unscoped
    ON api_route_bindings(team_id, api_definition_id, route_config_id)
    WHERE virtual_host IS NULL
      AND route IS NULL;
