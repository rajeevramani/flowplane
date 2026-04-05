---
name: flowplane-ops
description: Operate and troubleshoot Flowplane API gateway. Boot, start, set up, and initialize Flowplane in dev or prod mode. Diagnose routing failures, trace requests, validate configuration, audit changes, and monitor gateway health. Use when booting Flowplane, investigating gateway issues, performing health checks, setting up dev/prod mode, getting started, or understanding what changed.
license: Apache-2.0
metadata:
  author: rajeevramani
  version: "0.1.0"
compatibility: Requires Flowplane MCP server connection at /api/v1/mcp
---

# Flowplane Ops Skill

Boot, diagnose, inspect, and monitor a Flowplane API gateway. Sections 0 covers booting; sections 1-5 are **read-only** diagnostics.

## 0. Booting Flowplane

Flowplane has two auth modes. Choose based on your use case.

| | Dev Mode | Prod Mode (default) |
|---|---|---|
| **Use case** | Local dev, zero config | Multi-tenant, real auth |
| **Boot command** | `flowplane init --with-envoy --with-httpbin` | `make up ENVOY=1 HTTPBIN=1` |
| **Auth** | Opaque token (auto-generated) | Zitadel OIDC + JWT |
| **Seeding** | Automatic on startup | `make seed` after first boot |

**Prerequisite:** Run `make build` first if images aren't built yet.

### Dev Mode Boot
```bash
flowplane init --with-envoy --with-httpbin
# Starts PostgreSQL + control plane + Envoy + httpbin
# Token saved to ~/.flowplane/credentials
# API: localhost:8080, Envoy: localhost:10000, httpbin: localhost:8000
```

### Prod Mode Boot
```bash
make up ENVOY=1 HTTPBIN=1     # Start stack (auto-configures Zitadel on first run)
make seed                      # Seed demo data
make seed-info                 # Show credentials (admin@flowplane.local / Flowplane1!)
```

### Health Checks
```bash
# CLI
flowplane status               # System overview
flowplane doctor               # Diagnostic checks

# API
curl http://localhost:8080/api/v1/auth/mode
# Returns: {"auth_mode":"dev"} or {"auth_mode":"prod",...}

# MCP
devops_get_deployment_status { "include_details": true }
```

### Common Boot Failures

| Symptom | Cause | Fix |
|---|---|---|
| Network label conflict | Switched between dev/prod modes | `docker network rm flowplane-network`, retry |
| Port 8080 already in use | Previous stack not stopped | `make down` or `flowplane down`, retry |
| Image not found | Images not built | `make build` |
| Zitadel connection refused | Zitadel takes 30-60s on first boot | Wait, then `make setup-zitadel` |
| System won't start at all | Container crash | `docker ps -a` then `docker logs flowplane-control-plane` |
| DB connection refused | PostgreSQL not healthy | `docker logs flowplane-pg`, check port 5432 |
| `flowplane` command not found | CLI not built/on PATH | `cargo build --bin flowplane-cli` |

**Default credentials (prod):** `demo@acme-corp.com` / `Flowplane1!` (primary login), `admin@flowplane.local` / `Flowplane1!` (platform admin). Run `make seed-info` for all credentials.

See [references/boot-dev.md](references/boot-dev.md) and [references/boot-prod.md](references/boot-prod.md) for full recipes.

> For architecture details (auth internals, domain model, module map), see the `flowplane-dev` skill.

## 1. MCP Connection

**Endpoint:** `/api/v1/mcp` (Streamable HTTP, MCP protocol version `2025-11-25`)

**Authentication:** Bearer token in `Authorization` header. Required scope: `team:{name}:cp:read` (or org admin).

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
Args: { "path": "/api/v1/users" }
Args (with port filter): { "path": "/api/v1/users", "port": 10000 }
```

> **Params:** `path` (string, required), `port` (int, optional). No `method` or `host` params.

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
Args: { "resourceType": "clusters", "action": "delete", "limit": 10 }
```

> **Params:** All optional. `resourceType` (string), `action` (string: create/update/delete), `limit` (int, default 20, max 100). No `since`/`until`/`operation` params.

### `devops_get_deployment_status`

**What it does:** Returns aggregated health status across all clusters, listeners, and filters. Summary counts and optional details.

**When to use:** General health check, deployment verification.

```
Tool: devops_get_deployment_status
Args: { "includeDetails": true }
```

> **Params:** All optional. `clusterNames` (string array), `listenerNames` (string array), `filterNames` (string array), `includeDetails` (bool). Note: camelCase `includeDetails`, not `include_details`.

**CLI equivalent:** `flowplane status` (system overview) or `flowplane doctor` (diagnostic checks)

### `ops_xds_delivery_status`

**What it does:** Shows per-dataplane, per-resource-type (CDS/RDS/LDS/EDS) ACK/NACK status. For NACKed resources, includes the error message Envoy sent back explaining why it rejected the config.

**When to use:** Config looks correct in the DB but Envoy isn't applying it. Also useful as a first check in any troubleshooting workflow — if xDS delivery is broken, no config changes will take effect.

```
Tool: ops_xds_delivery_status
Args: { "dataplaneName": "my-envoy" }
```

> **Params:** All optional. `dataplaneName` (string). Note: camelCase `dataplaneName`, not `dataplane_name`.

**Output shows:** Each resource type's last ACK/NACK status, version info, and NACK error details.

### `ops_nack_history`

**What it does:** Queries recent NACK events — times when Envoy rejected a config push. Filterable by dataplane, resource type, and time range.

**When to use:** Investigating xDS delivery failures, finding which resource caused a NACK, understanding cascading failure patterns.

```
Tool: ops_nack_history
Args: { "dataplaneName": "my-envoy", "typeUrl": "CDS", "since": "2025-01-27T00:00:00Z", "limit": 10 }
```

> **Params:** All optional. `limit` (int, default 10, max 100), `dataplaneName` (string), `typeUrl` (string: CDS/RDS/LDS/EDS), `since` (ISO 8601 string). Note: camelCase `dataplaneName` and `typeUrl`, not snake_case.

**Output shows:** NACK events with timestamps, resource type, the rejected version, and error details.

### `cp_query_service`

**What it does:** Aggregate view of a single service: its cluster, endpoints, route configs that reference it, and listeners those route configs are bound to. Full picture for one service.

**When to use:** Understanding how a specific service is exposed.

```
Tool: cp_query_service
Args: { "name": "my-backend" }
```

> **Params:** `name` (string, required — the cluster name).

## 3. Troubleshooting Playbooks

### "Request returns 404"

A 404 means routing failed — the request didn't match any route.

1. **Trace the request:**
   ```
   ops_trace_request { "path": "/api/users" }
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

1. **Check for CDS NACKs:**
   ```
   ops_xds_delivery_status { "dataplaneName": "my-envoy" }
   ```
   If CDS is NACKed, cluster updates aren't reaching Envoy — see "Cascading xDS Failure" playbook.

2. **Check the cluster:**
   ```
   cp_get_cluster { "name": "my-backend" }
   ```
   Verify endpoints are correct and present.

3. **Check cluster health:**
   ```
   cp_get_cluster_health { "name": "my-backend" }
   ```

4. **Verify the route references the right cluster:**
   ```
   cp_get_route { "name": "my-route" }
   ```
   Check the `action.cluster` field.

5. **Check listener binding:**
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

1. **xDS delivery status:**
   ```
   ops_xds_delivery_status
   ```
   Check for any NACKs across dataplanes. NACKs mean config updates aren't reaching Envoy.

2. **Deployment status:**
   ```
   devops_get_deployment_status { "include_details": true }
   ```
   Check for any unhealthy clusters or missing resources.

3. **Config validation:**
   ```
   ops_config_validate
   ```
   Look for orphans, unbound configs, empty virtual hosts.

4. **Topology overview:**
   ```
   ops_topology
   ```
   Visual check for completeness and detect orphaned resources.

### "xDS Delivery Failure"

Config looks correct in the DB but Envoy isn't applying it. The control plane is pushing config, but Envoy is rejecting (NACKing) it.

1. **Check delivery status:**
   ```
   ops_xds_delivery_status { "dataplaneName": "my-envoy" }
   ```
   Look for any resource types showing NACK status. The error message tells you what Envoy rejected.

2. **Get NACK history:**
   ```
   ops_nack_history { "dataplaneName": "my-envoy", "typeUrl": "CDS", "limit": 10 }
   ```
   Find the first NACK — that's usually the root cause. Later NACKs may be cascading effects.

3. **Identify the bad resource:** The NACK error message typically names the specific resource (cluster, listener, route config) that has invalid configuration.

4. **Fix or remove the bad resource** using the `flowplane-api` skill, then verify delivery recovers:
   ```
   ops_xds_delivery_status { "dataplaneName": "my-envoy" }
   ```

### "Auth filter remote fetch failed"

An auth filter returns 401/403 and logs indicate it can't reach a remote dependency (JWKS provider, ext_authz service, OAuth2 token endpoint, etc.). The remote URI is correct, but Envoy can't connect.

**Key insight:** Auth filters that depend on upstream clusters (for JWKS fetch, ext_authz gRPC, etc.) will fail if those clusters aren't delivered. A cascading CDS NACK from a completely unrelated cluster blocks ALL cluster updates — including the ones auth filters depend on.

1. **Check for CDS NACKs:**
   ```
   ops_nack_history { "typeUrl": "CDS", "limit": 5 }
   ```
   If there's a NACK, the bad cluster is blocking dependent clusters from being updated.

2. **Check delivery status for the dataplane:**
   ```
   ops_xds_delivery_status { "dataplaneName": "my-envoy" }
   ```
   Confirm CDS is in NACK state.

3. **Identify and fix the bad cluster.** The NACK error names the offending cluster. Fix or remove it using the `flowplane-api` skill.

4. **Verify recovery:** Once the CDS NACK clears, Envoy will receive all pending cluster updates (including auth-related clusters) and the auth filter will start working.

### "Cascading xDS Failure"

One invalid resource is causing widespread failures across seemingly unrelated features.

**How it works:** Envoy processes xDS updates per resource type (CDS, RDS, LDS, EDS). When it NACKs an update, it rejects the **entire update for that resource type** — not just the bad resource. This means:
- One bad cluster → ALL cluster updates blocked (CDS NACK)
- One bad listener → ALL listener updates blocked (LDS NACK)
- One bad route config → ALL route config updates blocked (RDS NACK)

**Diagnosis:**
1. **Check all resource types:**
   ```
   ops_xds_delivery_status { "dataplaneName": "my-envoy" }
   ```

2. **Get NACK details for the affected type:**
   ```
   ops_nack_history { "dataplaneName": "my-envoy", "typeUrl": "CDS", "limit": 5 }
   ```

3. **Fix the root cause.** The first NACK event points to the original bad resource. Fix it and all pending updates for that resource type will be delivered.

**Common cascading patterns:**
- Bad cluster → auth filter remote dependencies unreachable → 401/403 on protected routes
- Bad cluster → new service clusters not delivered → 503 for new services
- Bad listener → filter chain updates stuck → new filters don't apply

### "Auth issues (401/403)"

Requests return 401 Unauthorized or 403 Forbidden.

0. **Check xDS delivery first:**
   ```
   ops_xds_delivery_status { "dataplaneName": "my-envoy" }
   ```
   If any resource type is NACKed, auth filters may not be working correctly. A CDS NACK can block clusters that auth filters depend on (JWKS providers, ext_authz backends). An LDS NACK can prevent filter chain updates from applying. Fix xDS delivery before investigating filter config — see "xDS Delivery Failure" playbook.

1. **Check filter chain:**
   ```
   cp_list_filters
   ```
   Look for JWT auth, ext_authz, RBAC, or OAuth2 filters.

2. **Check filter config:**
   ```
   cp_get_filter { "name": "my-jwt-auth" }
   ```
   Verify JWKS URIs, required issuers, and audiences.

3. **Check per-route overrides:**
   ```
   cp_get_route { "name": "my-route" }
   ```
   Check if `typedPerFilterConfig` overrides or disables the auth filter.

4. **Check filter attachment:**
   ```
   cp_list_filter_attachments { "filter_name": "my-jwt-auth" }
   ```
   Verify the filter is attached to the correct listener/route config.

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

### xDS Delivery
| Tool | Description |
|------|-------------|
| `ops_xds_delivery_status` | Per-dataplane ACK/NACK status for each resource type (CDS/RDS/LDS/EDS) |
| `ops_nack_history` | Query recent NACK events, filterable by dataplane, type, and time range |

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

- [references/boot-dev.md](references/boot-dev.md) — Full dev mode boot recipe with verification
- [references/boot-prod.md](references/boot-prod.md) — Full prod mode boot recipe with verification
- [references/architecture.md](references/architecture.md) — System architecture overview
- [references/troubleshooting.md](references/troubleshooting.md) — Expanded troubleshooting decision tree
