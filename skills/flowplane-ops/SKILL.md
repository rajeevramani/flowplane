---
name: flowplane-ops
description: Operate and troubleshoot Flowplane API gateway. Diagnose routing failures, trace requests, validate configuration, audit changes, and monitor gateway health. Use when investigating gateway issues, performing health checks, or understanding what changed.
license: Apache-2.0
metadata:
  author: rajeevramani
  version: "0.1.0"
compatibility: Requires Flowplane MCP server connection (control plane at /api/v1/mcp/cp)
---

# Flowplane Ops Skill

Diagnose, inspect, and monitor a Flowplane API gateway. This skill is **read-only** — it never modifies gateway configuration.

## 1. MCP Connection

**Endpoint:** `/api/v1/mcp/cp` (Streamable HTTP, MCP protocol version `2025-11-25`)

**Authentication:** Bearer token in `Authorization` header. Required scopes: `mcp:read`, `mcp:execute`, `cp:read`.

**Team Resolution:** Team context comes from:
1. `?team=<name>` query parameter
2. Token scopes matching `team:{name}:*`

> ⚠️ **Never hardcode team names.** Always derive from token or query parameter.

## 2. Diagnostic Tools

These are the primary tools for troubleshooting. Start here.

### `ops_trace_request`

**What it does:** Traces a request through the entire gateway path: listener → route config → virtual host → route → cluster → endpoints. Shows exactly where routing succeeds or fails.

**When to use:** 404s, unexpected routing, verifying a new route works.

```
Tool: ops_trace_request
Args: { "method": "GET", "path": "/api/v1/users", "host": "api.example.com" }
```

**Output shows:** Each resolution step with match/no-match status, the matched resources, and the final destination or failure point.

### `ops_topology`

**What it does:** Returns the complete gateway layout — all listeners, route configs, clusters, and their relationships. Detects orphaned resources (clusters not referenced by any route, unbound route configs).

**When to use:** Getting the big picture, finding orphans, onboarding to an unfamiliar gateway.

```
Tool: ops_topology
```

### `ops_config_validate`

**What it does:** Scans configuration for common misconfigurations:
- Orphan clusters (not referenced by any route)
- Unbound route configs (not attached to any listener)
- Empty virtual hosts (no routes defined)
- Duplicate path matchers (conflicting routes)

**When to use:** Proactive health checks, after bulk changes, CI/CD validation.

```
Tool: ops_config_validate
```

### `ops_audit_query`

**What it does:** Queries the audit log for recent operations — creates, updates, deletes. Shows what changed, when, and by whom.

**When to use:** Something broke recently, investigating who changed what.

```
Tool: ops_audit_query
Args: { "since": "2025-01-27T00:00:00Z", "operation": "update" }
```

### `devops_get_deployment_status`

**What it does:** Returns aggregated health status across all clusters, listeners, and filters. Summary counts and optional details.

**When to use:** General health check, deployment verification.

```
Tool: devops_get_deployment_status
Args: { "include_details": true }
```

### `cp_query_service`

**What it does:** Aggregate view of a single service: its cluster, endpoints, route configs that reference it, and listeners those route configs are bound to. Full picture for one service.

**When to use:** Understanding how a specific service is exposed.

```
Tool: cp_query_service
Args: { "cluster_name": "my-backend" }
```

## 3. Troubleshooting Playbooks

### "Request returns 404"

A 404 means routing failed — the request didn't match any route.

1. **Trace the request:**
   ```
   ops_trace_request { "method": "GET", "path": "/api/users", "host": "api.example.com" }
   ```
   Look at where the trace stops — this tells you which resolution step failed.

2. **If no listener matched:** Check that a listener exists on the expected port with `cp_list_listeners`.

3. **If no route config matched:** The listener may not have a route config bound. Check with `cp_get_listener`.

4. **If no virtual host matched:** The `Host` header doesn't match any domain. Check domains with `cp_list_virtual_hosts`.

5. **If no route matched:** Path matchers don't match the request path. Check routes with `cp_list_routes`. Common issues:
   - Exact match when prefix was intended
   - Missing leading `/`
   - Case sensitivity

### "Service unreachable"

Requests match a route but the upstream is unreachable.

1. **Check the cluster:**
   ```
   cp_get_cluster { "name": "my-backend" }
   ```
   Verify endpoints are correct and present.

2. **Check cluster health:**
   ```
   cp_get_cluster_health { "name": "my-backend" }
   ```

3. **Verify the route references the right cluster:**
   ```
   cp_get_route { "name": "my-route" }
   ```
   Check the `action.cluster` field.

4. **Check listener binding:**
   ```
   cp_get_listener { "name": "my-listener" }
   ```
   Verify the route config is bound to this listener.

### "What changed?"

Something broke and you need to find what was modified.

1. **Query recent audit events:**
   ```
   ops_audit_query { "since": "2025-01-27T10:00:00Z" }
   ```

2. **Filter by operation type:**
   ```
   ops_audit_query { "since": "2025-01-27T10:00:00Z", "operation": "delete" }
   ```

3. **Look at the resources involved** — the audit log shows resource type, name, and the operation performed.

### "General health check"

Proactive validation to catch issues before they affect traffic.

1. **Deployment status:**
   ```
   devops_get_deployment_status { "include_details": true }
   ```
   Check for any unhealthy clusters or missing resources.

2. **Config validation:**
   ```
   ops_config_validate
   ```
   Look for orphans, unbound configs, empty virtual hosts.

3. **Topology overview:**
   ```
   ops_topology
   ```
   Visual check for completeness and detect orphaned resources.

### "Filter not working"

A filter is configured but doesn't seem to apply.

1. **Check filter attachments:**
   ```
   cp_list_filter_attachments { "filter_name": "my-rate-limit" }
   ```
   Verify it's attached to the correct listener or route config.

2. **Check if filter is disabled:**
   ```
   cp_get_filter { "name": "my-rate-limit" }
   ```
   Look for `disabled: true` in the config.

3. **Check per-route overrides:** A route-level `typedPerFilterConfig` may be overriding or disabling the listener-level filter.

4. **Check filter order:** Filters execute in chain order. An auth filter after a rate limiter means unauthenticated requests get rate-limited before being rejected.

## 4. Resource Inspection Tools

These tools provide read-only access to all gateway resources.

### Clusters
| Tool | Description |
|------|-------------|
| `cp_list_clusters` | List all clusters (supports `limit`, `offset`) |
| `cp_get_cluster` | Get cluster config, endpoints, LB policy |
| `cp_get_cluster_health` | Get endpoint health status |

### Listeners
| Tool | Description |
|------|-------------|
| `cp_list_listeners` | List all listeners |
| `cp_get_listener` | Get listener config, filter chains, bound route configs |
| `cp_get_listener_status` | Get listener operational status |
| `cp_query_port` | Find what's listening on a specific port |

### Route Configs & Routes
| Tool | Description |
|------|-------------|
| `cp_list_route_configs` | List all route configurations |
| `cp_get_route_config` | Get route config with virtual hosts and routes |
| `cp_list_routes` | List all individual routes |
| `cp_get_route` | Get route details (matcher, action, filter overrides) |

### Virtual Hosts
| Tool | Description |
|------|-------------|
| `cp_list_virtual_hosts` | List virtual hosts (optionally filter by route config) |
| `cp_get_virtual_host` | Get virtual host domains and routes |

### Filters
| Tool | Description |
|------|-------------|
| `cp_list_filters` | List all HTTP filters |
| `cp_get_filter` | Get filter type, config, and enabled status |
| `cp_list_filter_attachments` | See where a filter is attached |
| `cp_list_filter_types` | List available filter types with schemas |

### Dataplanes
| Tool | Description |
|------|-------------|
| `cp_list_dataplanes` | List registered Envoy instances |
| `cp_get_dataplane` | Get dataplane config and connection status |

## 5. Key Principles

1. **Read-only.** The ops skill never creates, updates, or deletes resources. It only inspects and diagnoses. If a fix is needed, describe what should change and let the user (or the `flowplane-api` skill) make the modification.

2. **Start with context.** Before diving into specifics:
   - Use `ops_trace_request` for request-level issues
   - Use `ops_topology` for big-picture understanding
   - Use `devops_get_deployment_status` for health overview

3. **Check recent changes.** When something just broke, `ops_audit_query` is your first stop. Most issues are caused by recent modifications.

4. **Validate proactively.** Run `ops_config_validate` regularly to catch misconfigurations before they cause outages.

5. **Follow the data flow.** Requests flow: Listener → Route Config → Virtual Host → Route → Cluster → Endpoints. Trace this path to find where things break.

6. **`ops_trace_request` is a DB diagnostic.** It traces how routing *would* resolve based on stored config. It does NOT send real HTTP traffic through the gateway. For live testing, the user must send actual requests via curl or a traffic generator.

7. **Never hardcode team names.** Team context comes from the bearer token or `?team=` query parameter. Skills should be team-agnostic.

## References

- [references/architecture.md](references/architecture.md) — System architecture overview
- [references/troubleshooting.md](references/troubleshooting.md) — Expanded troubleshooting decision tree
