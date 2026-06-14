-- 0018: one virtual-host-scoped API route binding per API/route-config/vhost.
--
-- PostgreSQL UNIQUE constraints treat NULL route values as distinct, so 0010 still allowed
-- duplicate vhost-scoped bindings where route IS NULL. Keep the newest legacy duplicate,
-- then enforce the cardinality for the half-NULL scope.
WITH ranked_vhost_scoped AS (
    SELECT
        id,
        row_number() OVER (
            PARTITION BY team_id, api_definition_id, route_config_id, virtual_host
            ORDER BY created_at DESC, id DESC
        ) AS rank
    FROM api_route_bindings
    WHERE virtual_host IS NOT NULL
      AND route IS NULL
)
DELETE FROM api_route_bindings binding
USING ranked_vhost_scoped ranked
WHERE binding.id = ranked.id
  AND ranked.rank > 1;

CREATE UNIQUE INDEX idx_api_route_bindings_one_vhost_scope
    ON api_route_bindings(team_id, api_definition_id, route_config_id, virtual_host)
    WHERE virtual_host IS NOT NULL
      AND route IS NULL;
