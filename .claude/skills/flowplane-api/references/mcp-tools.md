# MCP Tools Reference

All 64 MCP tools grouped by category. Use via `tools/call` on the `/api/v1/mcp` endpoint.

> **Dev mode note:** All tools (except `cp_list_filter_types` and `cp_get_filter_type`) require an explicit `"team": "default"` argument in dev mode. This is by design: `dev_authenticate` creates an `AuthContext` with `org:dev-org:admin` scope but no grants, so `extract_teams()` returns empty. The `default` team exists in the DB (seeded by `seed_dev_resources` at startup with id `dev-default-team-id`), but must be specified explicitly via the `team` tool argument. This keeps dev mode simple — one org, one team, one user.

## Cluster Tools (7)

| Tool | Description | Required Params | Optional Params |
|------|-------------|----------------|-----------------|
| `cp_list_clusters` | List all clusters | — | `limit` (int), `offset` (int) |
| `cp_get_cluster` | Get cluster details + config | `name` (string) | — |
| `cp_get_cluster_health` | Endpoint health summary | `name` (string) | — |
| `cp_query_service` | Aggregate view: cluster + routes + listeners | `name` (string) | — |
| `cp_create_cluster` | Create cluster with endpoints | `name` (string), `serviceName` (string), `endpoints` (array of `{address, port}`) | `connectTimeoutSeconds` (int), `lbPolicy` (enum: ROUND_ROBIN/LEAST_REQUEST/RANDOM/RING_HASH/MAGLEV), `useTls` (bool), `healthCheck` (object), `circuitBreakers` (object) |
| `cp_update_cluster` | Update cluster config | `name` (string) | `serviceName`, `endpoints`, `connectTimeoutSeconds`, `lbPolicy`, `useTls`, `healthCheck`, `circuitBreakers` |
| `cp_delete_cluster` | Delete cluster | `name` (string) | — |

## Listener Tools (7)

| Tool | Description | Required Params | Optional Params |
|------|-------------|----------------|-----------------|
| `cp_list_listeners` | List all listeners | — | `limit` (int), `offset` (int) |
| `cp_get_listener` | Get listener config + filter chains | `name` (string) | — |
| `cp_get_listener_status` | Listener health + route config count | `name` (string) | — |
| `cp_query_port` | Check if port is in use | `port` (int) | — |
| `cp_create_listener` | Create listener | `name` (string), `port` (int), `dataplaneId` (string) | `address` (string, default "0.0.0.0"), `protocol` (enum: HTTP/HTTPS/TCP), `routeConfigName` (string), `filterChains` (array) |
| `cp_update_listener` | Update listener | `name` (string) | `address`, `port`, `protocol`, `routeConfigName`, `filterChains`, `dataplaneId` |
| `cp_delete_listener` | Delete listener | `name` (string) | — |

## Route Config Tools (5)

| Tool | Description | Required Params | Optional Params |
|------|-------------|----------------|-----------------|
| `cp_list_route_configs` | List all route configurations | — | `limit` (int), `offset` (int) |
| `cp_get_route_config` | Get route config details | `name` (string) | — |
| `cp_create_route_config` | Create route config with virtual hosts | `name` (string), `virtualHosts` (array) | — |
| `cp_update_route_config` | Full replacement of virtual hosts | `name` (string), `virtualHosts` (array) | — |
| `cp_delete_route_config` | Delete route config (cascades VHs + routes) | `name` (string) | — |

## Virtual Host Tools (5)

| Tool | Description | Required Params | Optional Params |
|------|-------------|----------------|-----------------|
| `cp_list_virtual_hosts` | List virtual hosts | — | `routeConfig` (string), `limit` (int), `offset` (int) |
| `cp_get_virtual_host` | Get virtual host details | `routeConfig` (string), `name` (string) | — |
| `cp_create_virtual_host` | Create virtual host in route config | `routeConfig` (string), `name` (string), `domains` (string array) | `ruleOrder` (int) |
| `cp_update_virtual_host` | Update virtual host | `routeConfig` (string), `name` (string) | `domains` (string array), `ruleOrder` (int) |
| `cp_delete_virtual_host` | Delete virtual host (cascades routes) | `routeConfig` (string), `name` (string) | — |

## Individual Route Tools (5)

| Tool | Description | Required Params | Optional Params |
|------|-------------|----------------|-----------------|
| `cp_list_routes` | List all routes across route configs | — | `routeConfig` (string), `limit` (int), `offset` (int) |
| `cp_get_route` | Get route by hierarchy | `routeConfig` (string), `virtualHost` (string), `name` (string) | — |
| `cp_create_route` | Create route in virtual host | `routeConfig` (string), `virtualHost` (string), `name` (string), `pathPattern` (string), `matchType` (enum: prefix/exact/regex/template), `action` (object) | `ruleOrder` (int) |
| `cp_update_route` | Partial update of route | `routeConfig` (string), `virtualHost` (string), `name` (string) | `pathPattern`, `matchType`, `ruleOrder`, `action`, `exposure` (enum: internal/external) |
| `cp_delete_route` | Delete route | `routeConfig` (string), `virtualHost` (string), `name` (string) | — |

## Query Tools (1)

| Tool | Description | Required Params | Optional Params |
|------|-------------|----------------|-----------------|
| `cp_query_path` | Check if path is routed | `path` (string) | `port` (int) |

> `cp_query_port` is listed under Listener Tools above.

## Filter Tools (5)

| Tool | Description | Required Params | Optional Params |
|------|-------------|----------------|-----------------|
| `cp_list_filters` | List all filters | — | `filterType` (enum: jwt_auth/oauth2/local_rate_limit/cors/header_mutation/ext_authz/rbac/custom_response/compressor/mcp), `limit`, `offset` |
| `cp_get_filter` | Get filter config + installations | `name` (string) | — |
| `cp_create_filter` | Create HTTP filter | `name` (string), `filterType` (string), `configuration` (object) | `description` (string) |
| `cp_update_filter` | Update filter config | `name` (string) | `newName` (string), `description` (string), `configuration` (object) |
| `cp_delete_filter` | Delete filter (must detach first) | `name` (string) | — |

## Filter Attachment Tools (3)

| Tool | Description | Required Params | Optional Params |
|------|-------------|----------------|-----------------|
| `cp_attach_filter` | Attach filter to listener or route config | `filter` (string) | `listener` (string), `routeConfig` (string), `order` (int), `settings` (object, route_config only) |
| `cp_detach_filter` | Detach filter from resource | `filter` (string) | `listener` (string), `routeConfig` (string) |
| `cp_list_filter_attachments` | List where a filter is attached | `filter` (string) | — |

## Filter Type Tools (2)

| Tool | Description | Required Params | Optional Params |
|------|-------------|----------------|-----------------|
| `cp_list_filter_types` | List all available filter types + schemas | — | — |
| `cp_get_filter_type` | Get filter type schema + config template | `name` (string) | — |

## Dataplane Tools (5)

| Tool | Description | Required Params | Optional Params |
|------|-------------|----------------|-----------------|
| `cp_list_dataplanes` | List registered dataplanes | — | `limit` (int), `offset` (int) |
| `cp_get_dataplane` | Get dataplane config | `team` (string), `name` (string) | — |
| `cp_create_dataplane` | Create a dataplane | `team` (string), `name` (string) | `gatewayHost` (string), `description` (string) |
| `cp_update_dataplane` | Update dataplane config | `team` (string), `name` (string) | `gatewayHost` (string), `description` (string) |
| `cp_delete_dataplane` | Delete a dataplane | `team` (string), `name` (string) | — |

## Learning Session Tools (5)

| Tool | Description | Required Params | Optional Params |
|------|-------------|----------------|-----------------|
| `cp_list_learning_sessions` | List learning sessions | — | `status` (enum: pending/active/completing/completed/cancelled/failed), `limit`, `offset` |
| `cp_get_learning_session` | Get learning session details | `id` (string, UUID) | — |
| `cp_create_learning_session` | Create learning session | `routePattern` (string), `targetSampleCount` (int) | `clusterName` (string), `httpMethods` (string array), `autoStart` (bool, default true) |
| `cp_activate_learning_session` | Activate a pending session | `id` (string, UUID) | — |
| `cp_delete_learning_session` | Cancel/delete a session | `id` (string, UUID) | — |

## Schema Tools (3)

| Tool | Description | Required Params | Optional Params |
|------|-------------|----------------|-----------------|
| `cp_list_aggregated_schemas` | List discovered API schemas | — | `path` (string), `httpMethod` (enum), `minConfidence` (float 0-1), `latestOnly` (bool), `limit`, `offset` |
| `cp_get_aggregated_schema` | Get schema details | `id` (int) | — |
| `cp_export_schema_openapi` | Export schemas as OpenAPI 3.1 | `schemaIds` (int array) | `title` (string), `version` (string), `description` (string), `includeMetadata` (bool) |

## OpenAPI Import Tools (2)

| Tool | Description | Required Params | Optional Params |
|------|-------------|----------------|-----------------|
| `cp_list_openapi_imports` | List OpenAPI import records | — | `limit` (int), `offset` (int) |
| `cp_get_openapi_import` | Get import details + spec content | `id` (string, UUID) | — |

## Ops / Diagnostic Tools (8)

| Tool | Description | Required Params | Optional Params |
|------|-------------|----------------|-----------------|
| `ops_trace_request` | Trace request path through gateway | `path` (string) | `port` (int) |
| `ops_topology` | Full gateway topology + orphan detection | — | `scope` (enum: listener/cluster/route_config), `name` (string), `limit` (int), `includeDetails` (bool) |
| `ops_config_validate` | Validate config for proto violations | — | — |
| `ops_audit_query` | Query recent audit log entries | — | `resourceType` (string), `action` (string), `limit` (int, max 100) |
| `ops_xds_delivery_status` | Per-dataplane ACK/NACK status | — | `dataplaneName` (string) |
| `ops_nack_history` | Query recent NACK events | — | `limit` (int), `dataplaneName` (string), `typeUrl` (string: CDS/RDS/LDS/EDS), `since` (ISO 8601 string) |
| `devops_get_deployment_status` | Aggregated deployment health | — | `clusterNames` (string array), `listenerNames` (string array), `filterNames` (string array), `includeDetails` (bool) |
| `dev_preflight_check` | Pre-creation validation | at least one of the optional params | `path` (string), `listenPort` (int), `clusterName` (string), `routeConfigName` (string), `listenerName` (string) |

## Learning Session Diagnostic (1)

| Tool | Description | Required Params | Optional Params |
|------|-------------|----------------|-----------------|
| `ops_learning_session_health` | Diagnose why session isn't collecting | `id` (string, UUID) | — |

## Known Issues (dev mode)

- **fp-dyg**: `cp_create_learning_session` always fails in dev mode (team resolution broken — MCP handler doesn't auto-resolve default team)
- **fp-dz5**: `cp_export_schema_openapi` fails in dev mode (org resolution broken)
- **fp-q8y**: `cp_list_aggregated_schemas` needs undocumented `team` param

## Parameter Naming Convention

All parameters use **camelCase**: `serviceName`, `dataplaneId`, `routeConfigName`, `filterType`, `listenPort`, `clusterName`, `routePattern`, `targetSampleCount`, `autoStart`, `schemaIds`, `includeMetadata`, `includeDetails`, `dataplaneName`, `typeUrl`, `httpMethod`, `minConfidence`, `latestOnly`, `pathPattern`, `matchType`, `ruleOrder`, `virtualHost`, `routeConfig`, `gatewayHost`, `filterChains`, `prefixRewrite`.

Exception: `team`, `name`, `path`, `port`, `filter`, `limit`, `offset`, `scope`, `since`, `action`, `order`, `domains` are single-word lowercase.
