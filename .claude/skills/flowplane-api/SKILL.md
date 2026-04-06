---
name: flowplane-api
description: Expose backend APIs through Flowplane's Envoy control plane. Create clusters, routes, listeners, and filters via MCP tools or CLI. Learn API schemas from live traffic and generate OpenAPI specs. Debug request routing and filter chains. Use when working with Flowplane MCP tools, CLI commands, or REST API for API configuration, terminal, or command line tasks.
license: Apache-2.0
metadata:
  author: rajeevramani
  version: "0.1.0"
compatibility: Requires Flowplane MCP server connection at /api/v1/mcp
---

# Flowplane API Skill

Configure and manage API gateway resources through Flowplane's MCP tools. This skill covers creating clusters, routes, listeners, and filters — plus learning API schemas from live traffic.

## 1. MCP Connection Setup

Flowplane exposes a unified MCP endpoint:

| Endpoint | Purpose | Scopes Required |
|----------|---------|----------------|
| `/api/v1/mcp` | All tools — control plane management and gateway API execution | `team:{name}:cp:read` / `team:{name}:cp:write` for CP tools; `team:{name}:api:read` / `team:{name}:api:execute` for gateway tools |

**Authentication:** Bearer token in `Authorization` header. All requests require a valid token.

### Auth Modes

Flowplane supports two authentication modes, detectable via `GET /api/v1/auth/mode`:

| Mode | Token Type | Setup | Team |
|------|-----------|-------|------|
| **Dev** (`FLOWPLANE_AUTH_MODE=dev`) | Opaque dev token (set via `FLOWPLANE_DEV_TOKEN` env var) | Zero config — `flowplane init` generates token, seeds org/team/dataplane | `default` (auto-created) |
| **Prod** (`FLOWPLANE_AUTH_MODE=prod`) | Zitadel JWT (identity) + DB grants (permissions) | Requires Zitadel, org/team setup | From DB-backed grants |

In dev mode, all requests authenticate as `dev@flowplane.local` with `org:dev-org:admin` scope. No Zitadel is required. The dev auth context has no grants, so `extract_teams()` returns empty — you must pass `"team": "default"` explicitly in every MCP tool call.

**Auth Mode Detection:** `GET /api/v1/auth/mode` (unauthenticated) returns:
- Dev: `{"auth_mode": "dev"}`
- Prod: `{"auth_mode": "prod", "oidc_issuer": "https://...", "oidc_client_id": "..."}`

**Team Resolution for MCP tools:** Team context is resolved from (in order):
1. `"team"` field in tool arguments (always works, required in dev mode)
2. `self.teams.first()` from the MCP session (populated from grants in prod mode)
3. Falls back to empty string (causes Forbidden errors)

In **dev mode**, always pass `"team": "default"` in tool arguments. The `default` team is auto-created by `seed_dev_resources` at startup.

In **prod mode**, team is derived from DB-backed grants (loaded via `load_permissions()` after JWT validation) — no explicit `team` arg needed.

**Protocol:** MCP 2025-11-25 over Streamable HTTP. Include `MCP-Protocol-Version: 2025-11-25` header. After `initialize`, include the `MCP-Session-Id` header from the response on all subsequent requests.

## 2. Domain Model

Flowplane translates high-level JSON configs into Envoy xDS resources. Understanding how resources connect is essential.

### Resource Types

| Resource | What It Is | Envoy Equivalent |
|----------|-----------|-----------------|
| **Cluster** | Backend service — endpoints + load balancing policy | CDS Cluster + EDS Endpoints |
| **Listener** | Entry point — address + port + filter chain | LDS Listener |
| **Route Config** | Routing rules — virtual hosts → routes → clusters | RDS RouteConfiguration |
| **Virtual Host** | Domain grouping within a route config — domains + routes | VirtualHost |
| **Route** | Single path match + action (forward, redirect, weighted) | Route |
| **Filter** | Policy (rate limiting, JWT auth, CORS, etc.) | HTTP Filter |
| **Dataplane** | Envoy proxy instance connected via xDS | Envoy node |

### Request Flow

```
Client Request
  → Listener (address:port, filter chain)
    → Route Config (bound to listener)
      → Virtual Host (matched by domain/Host header)
        → Route (matched by path)
          → Cluster (backend service)
            → Endpoints (actual servers)
```

Filters execute in order within the listener's filter chain. Per-route filter overrides can modify behavior for specific routes.

### Resource Relationships

- A **Listener** references one or more **Route Configs** (via `listener_route_configs`)
- A **Route Config** contains one or more **Virtual Hosts**
- A **Virtual Host** contains one or more **Routes**
- A **Route** references a **Cluster** (forward action) or multiple clusters (weighted action)
- **Filters** attach to **Listeners** or **Route Configs**

## 3. Naming Conventions

Use descriptive, unique names that include the service name, port, or path segment. **Never use generic names** like "my-cluster" or "test-listener".

| Resource | Pattern | Examples |
|----------|---------|----------|
| Cluster | `{service}-{port}-cluster` | `httpbin-8000-cluster`, `orders-api-3000-cluster` |
| Route Config | `{service}-{port}-rc` | `httpbin-10001-rc`, `orders-api-10002-rc` |
| Listener | `{service}-{port}-listener` | `httpbin-10001-listener`, `orders-10002-listener` |
| Virtual Host | `{service}-{port}-vhost` | `httpbin-10001-vhost`, `orders-10002-vhost` |

When deploying multiple services, names **must** differ. If a tool response contains `_dedup_warning`, report it to the user — the guardrail renamed a resource to avoid a collision.

## 4. Port Selection

Envoy containers typically expose ports **10000–10020**.

- **Dev mode port pool:** **10001–10020** (must match docker-compose port mapping). The expose API auto-assigns ports from this pool.
- **Always check availability first** with `cp_query_port` before choosing a port
- Prefer ports in the 10001–10020 range
- If a port is taken, pick the next available port in the range
- If a tool response contains `_port_warnings`, report them to the user

## 5. Smart Defaults

Use these defaults unless the user specifies otherwise:

| Setting | Default |
|---------|---------|
| Listen address | `0.0.0.0` |
| Protocol | HTTP |
| Load balancing | `ROUND_ROBIN` |
| Virtual host domains | `["*"]` |
| Match type | `prefix` |

## 6. Core Workflows

### 6.1 Quick Expose (REST API)

**Goal:** Expose a backend service in one API call. Use this for simple cases; use the manual MCP tool workflow (6.2) for advanced scenarios (filters, weighted routing, custom virtual hosts).

**Create:**
```
POST /api/v1/teams/{team}/expose
Authorization: Bearer {token}

Request:
{
  "name": "my-api",
  "upstream": "http://localhost:8000",
  "paths": ["/api/v1"],        // optional, default: ["/"]
  "port": 10005               // optional, auto-assigned from 10001-10020
}

Response:
{
  "name": "my-api",
  "upstream": "http://localhost:8000",
  "port": 10005,
  "paths": ["/api/v1"],
  "cluster": "my-api",
  "route_config": "my-api-routes",
  "listener": "my-api-listener"
}
```

**Delete:** `DELETE /api/v1/teams/{team}/expose/{name}` — tears down cluster, route config, and listener.

**Behavior:**
- **Idempotent:** repeat calls with same name+upstream return 200 (no new resources)
- **Conflict:** same name + different upstream → 409
- **Port pool:** 10001–10020 (auto-assigned if not specified; ports outside range → 400; collision → 409; exhausted → 409)
- **Naming:** cluster = `<name>`, route config = `<name>-routes`, listener = `<name>-listener`, virtual host = `<name>-routes-vhost` with domains `["*"]`
- **Upstream format:** `[http://]host:port[/path]` — scheme and path stripped, only host:port used for cluster endpoint. Port is required.
- **Delete:** cascades by naming convention — skips individual missing resources, returns 404 if none of the three existed

### 6.2 Manual Expose (MCP Tools)

**Goal:** Make a backend service accessible through the gateway with full control over each resource.

**Step 1: Check what already exists**
```
Tool: cp_list_listeners     — find existing listeners and used ports
Tool: cp_list_clusters      — avoid duplicate cluster names
Tool: cp_query_port         — check if desired port is available
```

**Step 2: Create a cluster** (backend service definition)
```
Tool: cp_create_cluster
Args: {
  "name": "httpbin-8000-cluster",
  "serviceName": "httpbin-service",
  "endpoints": [{"address": "httpbin", "port": 80}],
  "lb_policy": "ROUND_ROBIN"
}
```
Clusters also support optional resilience settings (pass as additional args):
- `circuitBreakers` — limit connections/requests/retries per priority level
- `healthChecks` — active health checking (HTTP or TCP probes)
- `outlierDetection` — passive health checking: eject hosts after consecutive 5xx errors (see section 6.2.1 below)

#### 6.2.1 Outlier Detection (Passive Health Checking)

Outlier detection monitors upstream endpoints and automatically ejects (removes from rotation) hosts that become unhealthy based on error rates.

**When to use:** For production services where you want Envoy to detect and avoid failing endpoints without active probes. Works especially well for HTTP services with error codes.

**Configuration fields:**

```
Tool: cp_create_cluster
Args: {
  "name": "my-api-cluster",
  "endpoints": [{"address": "api1", "port": 8000}, {"address": "api2", "port": 8000}, {"address": "api3", "port": 8000}],
  "outlierDetection": {
    "consecutive5xx": 5,           // Default: 5 (range: 1-1000)
    "intervalSeconds": 10,          // Default: 10 (range: 1-300)
    "baseEjectionTimeSeconds": 30,  // Default: 30 (range: 1-3600)
    "maxEjectionPercent": 10,       // Default: 10 (range: 1-100)
    "minHosts": 1                   // Default: 1 (range: 1-100)
  }
}
```

**Field meanings:**
- `consecutive5xx` — Number of consecutive 5xx responses needed before ejecting a host. Lower = faster ejection, higher = more tolerant.
- `intervalSeconds` — Time window (seconds) for measuring error rates. Checks run every N seconds.
- `baseEjectionTimeSeconds` — Minimum duration (seconds) a host remains ejected. After this time, Envoy will retry the host.
- `maxEjectionPercent` — Maximum percentage of cluster endpoints that can be ejected at once (prevents ejecting all healthy hosts). Value of 100 allows all hosts to be ejected.
- `minHosts` — Minimum number of healthy hosts required before allowing ejections. Prevents ejecting hosts when cluster is too small.

**Validation constraints:**
- `consecutive5xx`: 0 is invalid (must be ≥ 1)
- `intervalSeconds`: must be between 1 and 300
- `baseEjectionTimeSeconds`: must be between 1 and 3600
- `maxEjectionPercent`: must be between 1 and 100
- `minHosts`: must be between 1 and 100

**Example: Aggressive outlier detection for a flaky microservice**

```json
"outlierDetection": {
  "consecutive5xx": 3,              // Eject quickly (3 failures)
  "intervalSeconds": 5,             // Check frequently (every 5 sec)
  "baseEjectionTimeSeconds": 60,    // Keep ejected for 1 minute
  "maxEjectionPercent": 50,         // Don't eject more than 50% of hosts
  "minHosts": 2                     // Keep at least 2 hosts available
}
```

**Monitoring outlier detection:**

Check Envoy stats to verify ejections are working:
```
Tool: ops_get_envoy_stats
Args: {"cluster_name": "my-api-cluster"}
```

Look for stats like:
- `cluster.my-api-cluster.outlier_detection_ejections_active` — hosts currently ejected
- `cluster.my-api-cluster.outlier_detection_ejections_enforced` — enforced ejections
- `cluster.my-api-cluster.outlier_detection_ejections_total` — total ejections since startup

Outlier detection complements (but does not replace) active health checks — you can use both simultaneously.

**Step 3: Create a route config** (routing rules)
```
Tool: cp_create_route_config
Args: {
  "name": "httpbin-10001-rc",
  "virtualHosts": [{
    "name": "httpbin-10001-vhost",
    "domains": ["*"],
    "routes": [{
      "name": "all-traffic",
      "match": { "path": { "type": "prefix", "value": "/" } },
      "action": { "type": "forward", "cluster": "httpbin-8000-cluster" }
    }]
  }]
}
```

**Step 4: Create or reuse a listener**

> First call `cp_list_dataplanes` to get the dataplane UUID. In dev mode there is one dataplane named `dev-dataplane`.

```
Tool: cp_create_listener
Args: {
  "name": "httpbin-10001-listener",
  "address": "0.0.0.0",
  "port": 10001,
  "dataplaneId": "<from cp_list_dataplanes>"
}
```

> **Note:** If a listener already exists on the desired port, bind your route config to it instead of creating a new one.

**Step 5: Verify deployment**
```
Tool: cp_query_service       — full service view (cluster → routes → listener)
Tool: ops_config_validate    — check for misconfigurations
Tool: ops_trace_request      — trace a request through the gateway
```

Always verify after deployment. Report the listener port and a sample curl command to the user.

### 6.3 Add a Filter

**Goal:** Apply a policy (e.g., rate limiting) to a listener or route.

**Step 1: Discover available filter types**
```
Tool: cp_list_filter_types
```

**Step 2: Get the filter type schema** (understand required fields)
```
Tool: cp_get_filter_type
Args: { "name": "local_rate_limit" }
```

**Step 3: Create the filter** with ALL required fields
```
Tool: cp_create_filter
Args: {
  "name": "api-rate-limit",
  "filterType": "local_rate_limit",
  "config": {
    "type": "local_rate_limit",
    "config": {
      "stat_prefix": "api_rl",
      "token_bucket": {
        "max_tokens": 100,
        "tokens_per_fill": 100,
        "fill_interval_ms": 1000
      }
    }
  }
}
```

**Step 4: Check existing filter attachments** (pick a unique order)
```
Tool: cp_list_filter_attachments
Args: { "filter": "api-rate-limit" }
```
Also check what's already on the listener — each order value must be unique per listener.

**Step 5: Attach to a listener**
```
Tool: cp_attach_filter
Args: {
  "filter": "api-rate-limit",
  "listener": "httpbin-10001-listener",
  "order": 10
}
```

Or attach to a route config:
```
Tool: cp_attach_filter
Args: {
  "filter": "api-rate-limit",
  "route_config": "httpbin-10001-rc"
}
```

**Step 6: Verify attachment**
```
Tool: cp_get_filter
Args: { "name": "api-rate-limit" }
```
Check `listenerInstallations` or `routeConfigInstallations` arrays to confirm attachment. Note: the MCP `cp_get_filter` response includes these fields, but the CLI `flowplane filter get` output may not — use `cp_list_filter_attachments` as a fallback.

> **JWT Auth Filters:** MUST include `rules` with match patterns. A JWT filter without rules does NOT enforce authentication — it passes all traffic through unauthenticated. Always include rules like:
> ```json
> "rules": [{"match": {"prefix": "/api"}, "requires": {"type": "provider_name", "provider_name": "my-provider"}}]
> ```

> **Secrets (auth filters):** Filters that need credentials (`oauth2`, `jwt_auth`, `ext_authz`) can reference secrets by name. Note: `credential_injector` has an XDS module but is NOT a registered FilterType. Create the secret first, then reference it in the filter config. See the `flowplane-secrets` skill for the full workflow, CLI commands, MCP tools, and secret types.

### 6.4 Learn API from Traffic

**Goal:** Discover API schemas by observing live traffic, then export as OpenAPI.

#### Learning Session Lifecycle
- **Pending** — Created with `auto_start=false`, waiting to be activated
- **Active** — Collecting traffic samples through the gateway
- **Completing** — Target sample count reached, generating schema
- **Completed** — Schema generation finished, results available
- **Cancelled** — Manually cancelled via `cp_delete_learning_session`
- **Failed** — Error occurred (check `error_message`)

#### Sample Count Guidance
| Scenario | Samples | Use Case |
|----------|---------|----------|
| Quick test | 5–20 | Verify the learning pipeline works |
| Simple CRUD | 20–100 | GET/POST/PUT/DELETE with consistent schemas |
| Complex APIs | 100–500 | Multiple response shapes, nested objects |
| High variance | 500+ | Dynamic fields, polymorphic responses |

Sessions can collect more samples than the target (e.g., 200% progress). The session completes once it processes at least `targetSampleCount` samples, but samples arriving in the same collection window are all captured.

**Step 1: Discover the gateway layout**
```
Tool: cp_list_route_configs   — see configured paths
Tool: cp_list_routes          — find route patterns
Tool: cp_list_listeners       — find the gateway port
```

**Step 2: Check for existing sessions**
```
Tool: cp_list_learning_sessions
```
If an active session already covers the target routes, skip to monitoring. If completed, skip to schema review.

**Step 3: Create a learning session**
```
Tool: cp_create_learning_session
Args: {
  "routePattern": "^/api/v1/.*",
  "targetSampleCount": 50,
  "team": "default",
  "name": "my-api-v1",
  "httpMethods": ["GET", "POST", "PUT", "DELETE"],
  "autoStart": true,
  "autoAggregate": false
}
```
- `name` — Human-readable session name. If omitted, auto-generated from route_pattern. Used for name-based lookups in all subsequent tools.
- `autoAggregate` — Enable snapshot mode: periodic aggregation while continuing to collect. Default: false.
- The `team` parameter is recommended via MCP in dev mode to scope the session to the correct team.

All learning session tools (`cp_get_learning_session`, `cp_stop_learning`, `cp_activate_learning_session`, `cp_delete_learning_session`) accept either session name or UUID for the `id` parameter.

**Step 3b: Verify activation**
```
Tool: cp_get_learning_session
Args: { "id": "<session-id>" }
```
Confirm `status == "active"` and `started_at` is not null. If status is `pending`, the session is not collecting — recreate with `auto_start: true`.

**Step 4: Tell the user to generate traffic**

> **`ops_trace_request` is a DB diagnostic — it does NOT generate real HTTP traffic.**

After discovering the listener port and route paths, provide concrete curl examples:
```
Send traffic to http://localhost:{port}{path} — I need {target} samples.
Example: curl http://localhost:10001/api/v1/users
```

**Step 5: Monitor progress**
```
Tool: cp_get_learning_session
Args: { "id": "<session-id>" }
```
Report progress as `current_sample_count / target_sample_count (percentage)`. If samples aren't increasing, diagnose systematically:

| Symptom | Diagnosis | Fix |
|---------|-----------|-----|
| `status: pending`, `started_at: null` | Session never activated | Recreate with `auto_start: true` or call `cp_activate_learning_session` |
| `status: active`, `current_sample_count: 0` | Regex doesn't match traffic paths | Check route_pattern against actual request paths. Use `ops_learning_session_health` |
| `status: active`, samples increasing slowly | Traffic volume is low | Send more requests or wait longer |
| `status: failed` | Error during collection | Check `error_message` field for details |

**Step 6: View discovered schemas**
```
Tool: cp_list_aggregated_schemas
Args: { "team": "default" }
```
The `team` parameter is required in dev mode (known inconsistency — other list tools work without it).

Report: path, HTTP method, confidence score, sample count. Flag low-confidence schemas (below 0.5) as needing more traffic. Schema output includes field-level `type`, `format` (e.g., `ipv4`, `uri`), `required` arrays, and `presence_count` per field.

#### Confidence Score Interpretation
| Score | Meaning |
|-------|---------|
| 0.9–1.0 | Very reliable — many consistent samples |
| 0.7–0.9 | Good — minor variations observed |
| 0.5–0.7 | Moderate — some inconsistency, review recommended |
| Below 0.5 | Low — needs more traffic or has high variance |

**Step 7: Export as OpenAPI**
```
Tool: cp_export_schema_openapi
Args: { "schemaIds": [1, 2], "team": "default", "title": "My API", "version": "1.0.0" }
```
Note: `schemaIds` is camelCase and takes integer IDs (not strings). The `team` parameter is required in dev mode. Output is OpenAPI 3.1.0 with inferred response schemas, header parameters, and content types. Export uses structural fingerprinting to deduplicate shared schemas as `$ref` references in `components/schemas`.

Or use the CLI:
```bash
flowplane schema export --all -o api.yaml
flowplane learn export --session <name> -o api.yaml
```

### 6.5 Blue/Green Deployment

**Goal:** Split traffic between two backend versions.

Create two clusters, then use a weighted route action:

```
Tool: cp_create_route_config
Args: {
  "name": "blue-green-routes",
  "virtualHosts": [{
    "name": "app",
    "domains": ["app.example.com"],
    "routes": [{
      "name": "weighted-split",
      "match": { "path": { "type": "prefix", "value": "/" } },
      "action": {
        "type": "weighted",
        "totalWeight": 100,
        "clusters": [
          { "name": "app-v1", "weight": 90 },
          { "name": "app-v2", "weight": 10 }
        ]
      }
    }]
  }]
}
```

Gradually shift weight from v1 to v2 by updating the route config.

### 6.6 Debug a Request

**Goal:** Understand why a request isn't routing correctly.

1. **Check routes:** `cp_list_routes` — verify path matchers and cluster references
2. **Check cluster health:** `cp_get_cluster` — verify endpoints are present
3. **Check filter chain:** `cp_list_filters` — verify no filter is blocking
4. **Check listener binding:** `cp_get_listener` — verify route config is bound
5. **Trace the request:** `ops_trace_request` (from flowplane-ops skill) — full path analysis

## 7. Error Handling & Recovery

When a tool call fails, **do not retry with the same parameters**. Diagnose first:

| Error | Action |
|-------|--------|
| **ALREADY_EXISTS** | Query the existing resource with `cp_get_*`. If it matches what you need, reuse it. Otherwise, generate a new unique name (append port or path segment) and retry. |
| **NOT_FOUND** | List resources with `cp_list_*` to find the correct name, then retry with the corrected reference. |
| **CONFLICT** | Get the conflicting resource details with `cp_get_*` before retrying. Understand what conflicts before choosing a resolution. |
| **Validation error** | Use `cp_get_filter_type` to understand the correct schema. When retrying, preserve ALL required fields — never silently drop fields like `rules`. |

## 8. Path Matchers

Routes match requests by path. Four matcher types are available:

| Type | JSON | Use Case |
|------|------|----------|
| **Exact** | `{ "type": "exact", "value": "/health" }` | Health probes, specific files |
| **Prefix** | `{ "type": "prefix", "value": "/api" }` | API namespaces, broad matching |
| **Regex** | `{ "type": "regex", "value": "^/v[0-9]+/" }` | Version guards, complex patterns |
| **Template** | `{ "type": "template", "template": "/users/{user_id}" }` | RESTful resource paths with captures |

**Template rewrites:** Combine template match with `templateRewrite` to reshape paths:
```json
{
  "match": { "path": { "type": "template", "template": "/api/v1/users/{user_id}" } },
  "action": {
    "type": "forward",
    "cluster": "users-internal",
    "templateRewrite": "/internal/{user_id}/profile"
  }
}
```

> ⚠️ Header and query parameter matchers are accepted by the API but **not yet wired in xDS**. Stick to path-based matching.

## 9. Available Filters

| Filter | Envoy Name | Per-Route | Description |
|--------|-----------|-----------|-------------|
| OAuth2 | `envoy.filters.http.oauth2` | No | OAuth2 authorization code flow |
| JWT Auth | `envoy.filters.http.jwt_authn` | Yes | JWT validation with JWKS providers |
| Ext Auth | `envoy.filters.http.ext_authz` | Yes | External authorization service |
| RBAC | `envoy.filters.http.rbac` | Yes | Role-based access control |
| Local Rate Limit | `envoy.filters.http.local_ratelimit` | Yes | In-process token bucket rate limiting |
| Rate Limit | `envoy.filters.http.ratelimit` | Yes | Distributed rate limiting |
| ~~Rate Limit Quota~~ | `envoy.filters.http.rate_limit_quota` | N/A | **NOT a FilterType** — XDS module exists but not in enum |
| CORS | `envoy.filters.http.cors` | Yes | Cross-origin resource sharing |
| Header Mutation | `envoy.filters.http.header_mutation` | Yes | Request/response header manipulation |
| Custom Response | `envoy.filters.http.custom_response` | Yes | Custom error responses |
| ~~Health Check~~ | `envoy.filters.http.health_check` | N/A | **NOT a FilterType** — XDS module exists but not in enum |
| ~~Credential Injector~~ | `envoy.filters.http.credential_injector` | N/A | **NOT a FilterType** — XDS module exists but not in enum |
| ~~Ext Proc~~ | `envoy.filters.http.ext_proc` | N/A | **NOT a FilterType** — XDS module exists but not in enum |
| Compressor | `envoy.filters.http.compressor` | Yes | Response compression (gzip, brotli) |

See [references/filters-quick-ref.md](references/filters-quick-ref.md) for config examples.

## 10. Per-Route Filter Overrides

Filters with per-route support can be overridden using `typedPerFilterConfig` on:
- **Routes** — override for a specific path
- **Virtual Hosts** — override for a domain group
- **Weighted Clusters** — override per backend in weighted routing

The key is the Envoy filter name; the value is the filter-specific config:

```json
"typedPerFilterConfig": {
  "envoy.filters.http.local_ratelimit": {
    "stat_prefix": "per_route",
    "token_bucket": {
      "max_tokens": 10,
      "tokens_per_fill": 10,
      "fill_interval_ms": 1000
    }
  },
  "envoy.filters.http.jwt_authn": {
    "requirement_name": "allow_missing"
  }
}
```

### Disabling a Filter for a Specific Route

To **completely disable** a filter on a route, use `"disabled": true`. This works for any filter with per-route support:

```json
"typedPerFilterConfig": {
  "envoy.filters.http.jwt_authn": {
    "disabled": true
  }
}
```

This is different from `"requirement_name": "allow_missing"` — `disabled` skips the filter entirely, while `allow_missing` still runs the filter but doesn't reject requests without a token.

### Exempting a Sub-Path from a Filter

Common scenario: a filter (JWT, rate limit, etc.) protects a broad prefix, but a specific sub-path needs different treatment (e.g. health checks, public endpoints, webhooks).

**The key insight:** Add a **more-specific route before the broader one** in the same virtual host. Envoy evaluates routes top-to-bottom and uses the first match.

**Step 1:** Fetch the existing route config with `cp_get_route_config` to see all current routes.

**Step 2:** Use `cp_update_route_config` to add the exempt route **before** the protected route. `cp_update_route_config` performs a **full replacement** of the `virtualHosts` array — include ALL existing routes plus the new one.

```json
{
  "name": "<route-config-name>",
  "virtualHosts": [{
    "name": "<vhost-name>",
    "domains": ["*"],
    "routes": [
      {
        "name": "<exempt-route-name>",
        "match": { "path": { "type": "prefix", "value": "<more-specific-path>" } },
        "action": { "type": "forward", "cluster": "<cluster-name>" },
        "typedPerFilterConfig": {
          "<envoy-filter-name>": { "disabled": true }
        }
      },
      {
        "name": "<protected-route-name>",
        "match": { "path": { "type": "prefix", "value": "<broader-path>" } },
        "action": { "type": "forward", "cluster": "<cluster-name>" }
      }
    ]
  }]
}
```

> **Route ordering matters.** The more-specific route MUST come before the broader prefix in the routes array. If reversed, the broad prefix matches first and the filter applies to everything.

> **Full replacement.** Always fetch existing config first, preserve all existing routes, then insert the new exempt route at the correct position.

This allows different rate limits, auth requirements, or processing modes per endpoint while keeping defaults at the listener level.

### REST API for Per-Route Overrides

In addition to `typedPerFilterConfig` in route config JSON, Flowplane provides a REST API for managing per-route overrides:

**Override config on a route:**
```
POST /api/v1/filters/{filterId}/configurations
{
  "scopeType": "route",
  "scopeId": "route-config-name/vhost-name/route-name",
  "settings": {
    "behavior": "override",
    "config": { ... filter-specific config ... }
  }
}
```

**Disable filter on a route:**
```
POST /api/v1/filters/{filterId}/configurations
{
  "scopeType": "route",
  "scopeId": "route-config-name/vhost-name/route-name",
  "settings": {
    "behavior": "disable"
  }
}
```

**Scope at route-config level:**
```
POST /api/v1/filters/{filterId}/configurations
{
  "scopeType": "route-config",
  "scopeId": "my-route-config"
}
```

The `scopeId` for routes uses the format `{route-config-name}/{vhost-name}/{route-name}`.

## 11. Common Pitfalls

| Pitfall | Details |
|---------|---------|
| **Router filter is auto-appended** | Don't add `envoy.filters.http.router` manually — Flowplane appends it automatically. Adding it yourself causes a validation error. |
| **Header/query matching not wired** | The API accepts header and query param matchers but they are **ignored in xDS generation**. Use path matchers only. |
| **Team from token** | Team context comes from the bearer token scopes or `?team=` param. Never hardcode team names in configs. |
| **Cluster must exist first** | A cluster must exist before you reference it in a route action. Create clusters before route configs. |
| **Filter attachment order** | Filters execute in the order they appear in the listener's filter chain. Auth filters should come before rate limiters. Each order value must be unique per listener — use `cp_list_filter_attachments` to check existing orders before attaching. |
| **Route config must be bound** | Creating a route config doesn't automatically expose it. It must be bound to a listener (via `listener_route_configs`). |
| **Route ordering is first-match** | Envoy evaluates routes top-to-bottom in the `routes` array. More-specific paths (e.g. `/api/accounts`) must come before broader prefixes (e.g. `/api`). |
| **`cp_update_route_config` is full replacement** | Updating a route config replaces the entire `virtualHosts` array. Always fetch existing config with `cp_get_route_config` first, preserve all existing routes, then add new ones. |
| **`disabled: true` vs `allow_missing`** | `"disabled": true` completely skips the filter. `"requirement_name": "allow_missing"` still runs the filter but accepts missing credentials. Use `disabled` to fully exempt a route. |
| **Filter config nesting** | Filter create uses `"config": {"type": "filter_type", "config": {...}}` — not a flat `"configuration"` field. The `type` inside config must match the `filterType` field. |

## 12. CLI Equivalents

Every MCP tool operation has a CLI equivalent. Use the CLI for scripting, CI/CD, and local development.

### Quick Actions (one-command shortcuts)

| Operation | CLI | MCP Equivalent |
|---|---|---|
| Expose a service | `flowplane expose <upstream> --name <name> --port <port>` | `POST /api/v1/teams/{team}/expose` |
| Remove exposed service | `flowplane unexpose <name>` | `DELETE /api/v1/teams/{team}/expose/{name}` |
| Import OpenAPI | `flowplane import openapi <file> --name <name>` | (no single MCP tool — uses create sequence) |
| List exposed services | `flowplane list` | (no direct MCP equivalent) |

### Resource CRUD

| MCP Tool | CLI Equivalent |
|---|---|
| `cp_create_cluster` | `flowplane cluster create -f cluster.json` |
| `cp_list_clusters` | `flowplane cluster list` |
| `cp_get_cluster` | `flowplane cluster get <name>` |
| `cp_update_cluster` | `flowplane cluster update -f cluster.json` |
| `cp_delete_cluster` | `flowplane cluster delete <name>` |
| `cp_create_listener` | `flowplane listener create -f listener.json` |
| `cp_list_listeners` | `flowplane listener list` |
| `cp_get_listener` | `flowplane listener get <name>` |
| `cp_delete_listener` | `flowplane listener delete <name>` |
| `cp_create_route_config` | `flowplane route create -f route-config.json` |
| `cp_list_route_configs` | `flowplane route list` |
| `cp_get_route_config` | `flowplane route get <name>` |
| `cp_update_route_config` | `flowplane route update -f route-config.json` |
| `cp_delete_route_config` | `flowplane route delete <name>` |
| `cp_create_filter` | `flowplane filter create -f filter.json` |
| `cp_list_filters` | `flowplane filter list` |
| `cp_get_filter` | `flowplane filter get <name>` |
| `cp_delete_filter` | `flowplane filter delete <name>` |
| `cp_attach_filter` | `flowplane filter attach <name> --listener <listener> --order <n>` |
| `cp_detach_filter` | `flowplane filter detach <name> --listener <listener>` |

### Learning & Schema

| MCP Tool | CLI Equivalent |
|---|---|
| `cp_create_learning_session` | `flowplane learn start --route-pattern <regex> --target-sample-count <n> [--name <name>] [--auto-aggregate]` |
| `cp_list_learning_sessions` | `flowplane learn list` |
| `cp_get_learning_session` | `flowplane learn get <name-or-id>` |
| `cp_stop_learning` | `flowplane learn stop <name-or-id>` |
| `cp_delete_learning_session` | `flowplane learn cancel <name-or-id> --yes` |
| `cp_list_aggregated_schemas` | `flowplane schema list [--min-confidence N] [--session <name>] [--path <path>] [--method <method>]` |
| `cp_get_aggregated_schema` | `flowplane schema get <id>` |
| `cp_export_schema_openapi` | `flowplane schema export --id 1,2,3` or `flowplane schema export --all` |
| (all schemas shortcut) | `flowplane learn export [--session <name>]` |

### Secrets

MCP tools: `cp_create_secret`, `cp_list_secrets`, `cp_get_secret`, `cp_delete_secret`. See the **`flowplane-secrets` skill** for full details.

### Diagnostics

| MCP Tool | CLI Equivalent |
|---|---|
| `devops_get_deployment_status` | `flowplane status` |
| (system diagnostics) | `flowplane doctor` |
| (log inspection) | `flowplane logs [-f]` |

> For full CLI flag details, see the `flowplane-cli` skill.

## References

- [references/envoy-concepts.md](references/envoy-concepts.md) — Envoy semantics: xDS dependencies, route matching, per-route overrides, NACK causes, health checks
- [references/routing-cookbook.md](references/routing-cookbook.md) — Route patterns: forward, weighted, redirects, templates, per-route filters
- [references/mcp-tools.md](references/mcp-tools.md) — All MCP tool names, descriptions, and key parameters
- [references/filters-quick-ref.md](references/filters-quick-ref.md) — Filter config examples for common filters
