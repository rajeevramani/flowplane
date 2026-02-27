# MCP Tools Reference

All control plane MCP tools grouped by category. Use via `tools/call` on the `/api/v1/mcp/cp` endpoint.

## Read / Query Tools

| Tool | Description | Key Parameters |
|------|-------------|---------------|
| `cp_list_clusters` | List all clusters | `limit`, `offset` |
| `cp_get_cluster` | Get cluster details | `name` (required) |
| `cp_list_listeners` | List all listeners | `limit`, `offset` |
| `cp_get_listener` | Get listener config with filters | `name` (required) |
| `cp_list_route_configs` | List all route configurations | `limit`, `offset` |
| `cp_get_route_config` | Get route config details | `name` (required) |
| `cp_list_routes` | List all routes | `limit`, `offset` |
| `cp_get_route` | Get route details | `name` (required) |
| `cp_list_virtual_hosts` | List virtual hosts | `route_config_name` |
| `cp_get_virtual_host` | Get virtual host details | `name` (required) |
| `cp_list_filters` | List all HTTP filters | `limit`, `offset` |
| `cp_get_filter` | Get filter config | `name` (required) |
| `cp_list_filter_attachments` | List filter attachments | `filter_name` (required) |
| `cp_list_filter_types` | List available filter types | — |
| `cp_get_filter_type` | Get filter type schema | `name` (required) |
| `cp_query_service` | Aggregate view: cluster + endpoints + routes + listeners | `cluster_name` (required) |
| `cp_query_path` | Query what matches a given path | `path` (required) |

## Cluster Operations

| Tool | Description | Key Parameters |
|------|-------------|---------------|
| `cp_create_cluster` | Create cluster with endpoints | `name`, `endpoints[]`, `lb_policy` |
| `cp_update_cluster` | Update cluster config | `name`, fields to update |
| `cp_delete_cluster` | Delete cluster (fails if referenced) | `name` (required) |
| `cp_get_cluster_health` | Get cluster endpoint health | `name` (required) |

## Listener Operations

| Tool | Description | Key Parameters |
|------|-------------|---------------|
| `cp_create_listener` | Create listener | `name`, `address`, `port`, `protocol` |
| `cp_update_listener` | Update listener config | `name`, fields to update |
| `cp_delete_listener` | Delete listener | `name` (required) |
| `cp_get_listener_status` | Get listener status | `name` (required) |
| `cp_query_port` | Query what's on a port | `port` (required) |

## Route Operations

| Tool | Description | Key Parameters |
|------|-------------|---------------|
| `cp_create_route_config` | Create route config with virtual hosts | `name`, `virtualHosts[]` |
| `cp_update_route_config` | Update route config | `name`, fields to update |
| `cp_delete_route_config` | Delete route config | `name` (required) |
| `cp_create_route` | Create individual route | `virtual_host_name`, route definition |
| `cp_update_route` | Update route | `name`, fields to update |
| `cp_delete_route` | Delete route | `name` (required) |
| `cp_create_virtual_host` | Create virtual host | `route_config_name`, `name`, `domains[]` |
| `cp_update_virtual_host` | Update virtual host | `name`, fields to update |
| `cp_delete_virtual_host` | Delete virtual host | `name` (required) |

## Filter Operations

| Tool | Description | Key Parameters |
|------|-------------|---------------|
| `cp_create_filter` | Create HTTP filter | `name`, `filter_type`, `config` |
| `cp_update_filter` | Update filter config | `name`, fields to update |
| `cp_delete_filter` | Delete filter | `name` (required) |
| `cp_attach_filter` | Attach filter to resource | `filter_name`, `resource_type`, `resource_name` |
| `cp_detach_filter` | Detach filter from resource | `filter_name`, `resource_type`, `resource_name` |

## Learning & Schema Tools

| Tool | Description | Key Parameters |
|------|-------------|---------------|
| `cp_create_learning_session` | Start traffic learning session | `route_pattern`, `target_samples` |
| `cp_get_learning_session` | Get learning session status | `id` (required) |
| `cp_list_learning_sessions` | List all learning sessions | `limit`, `offset` |
| `cp_delete_learning_session` | Delete learning session | `id` (required) |
| `cp_list_aggregated_schemas` | List discovered API schemas | `limit`, `offset` |
| `cp_get_aggregated_schema` | Get schema details | `id` (required) |
| `cp_export_schema_openapi` | Export schemas as OpenAPI spec | `schema_ids[]` (required) |
| `cp_list_openapi_imports` | List OpenAPI imports | `limit`, `offset` |
| `cp_get_openapi_import` | Get import details | `id` (required) |

## Dataplane Tools

| Tool | Description | Key Parameters |
|------|-------------|---------------|
| `cp_list_dataplanes` | List registered dataplanes | `limit`, `offset` |
| `cp_get_dataplane` | Get dataplane config | `name` (required) |
| `cp_create_dataplane` | Register a dataplane | `name`, config fields |
| `cp_update_dataplane` | Update dataplane config | `name`, fields to update |
| `cp_delete_dataplane` | Unregister a dataplane | `name` (required) |

## Ops / Diagnostic Tools

| Tool | Description | Key Parameters |
|------|-------------|---------------|
| `ops_trace_request` | Trace request path through gateway | `method`, `path`, `host` |
| `ops_topology` | Full gateway topology with orphan detection | — |
| `ops_config_validate` | Validate config for misconfigurations | — |
| `ops_audit_query` | Query audit log for recent changes | `since`, `until`, `operation` |
| `devops_get_deployment_status` | Aggregated health status | `include_details` |
| `dev_preflight_check` | Pre-deployment validation | `cluster_name` |
