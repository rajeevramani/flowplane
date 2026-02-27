---
name: flowplane-api
description: Expose backend APIs through Flowplane's Envoy control plane. Create clusters, routes, listeners, and filters. Learn API schemas from live traffic and generate OpenAPI specs. Debug request routing and filter chains. Use when working with Flowplane MCP tools for API configuration.
license: Apache-2.0
metadata:
  author: rajeevramani
  version: "0.1.0"
compatibility: Requires Flowplane MCP server connection (control plane at /api/v1/mcp/cp, gateway API at /api/v1/mcp/api)
---

# Flowplane API Skill

Configure and manage API gateway resources through Flowplane's MCP tools. This skill covers creating clusters, routes, listeners, and filters — plus learning API schemas from live traffic.

## 1. MCP Connection Setup

Flowplane exposes two MCP endpoints:

| Endpoint | Purpose | Scopes Required |
|----------|---------|----------------|
| `/api/v1/mcp/cp` | Control plane tools — manage clusters, routes, listeners, filters | `mcp:read`, `mcp:execute`, `cp:read` |
| `/api/v1/mcp/api` | Gateway API tools — invoke backend APIs through Envoy | `api:read`, `api:execute` |

**Authentication:** Bearer token in `Authorization` header. All requests require a valid token.

**Team Resolution:** Team context is resolved from:
1. `?team=<name>` query parameter (explicit)
2. Token scopes matching `team:{name}:*` (implicit)

> ⚠️ **Never hardcode team names.** Team is always derived from the token or query parameter.

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

1. **Always check availability first** with `cp_query_port` before choosing a port
2. Prefer ports in the 10000–10020 range
3. If a port is taken, pick the next available port in the range
4. If a tool response contains `_port_warnings`, report them to the user

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

### 6.1 Expose a Service

**Goal:** Make a backend service accessible through the gateway.

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
  "endpoints": ["host.docker.internal:8000"],
  "lb_policy": "ROUND_ROBIN"
}
```

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
```
Tool: cp_create_listener
Args: {
  "name": "httpbin-10001-listener",
  "address": "0.0.0.0",
  "port": 10001
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

### 6.2 Add a Filter

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
  "filter_type": "local_rate_limit",
  "config": {
    "stat_prefix": "api_rl",
    "token_bucket": {
      "max_tokens": 100,
      "tokens_per_fill": 100,
      "fill_interval_ms": 1000
    }
  }
}
```

**Step 4: Attach to a resource**
```
Tool: cp_attach_filter
Args: {
  "filter_name": "api-rate-limit",
  "resource_type": "listener",
  "resource_name": "httpbin-10001-listener"
}
```

**Step 5: Verify attachment**
```
Tool: cp_get_filter
Args: { "name": "api-rate-limit" }
```
Check `listenerInstallations` or `routeConfigInstallations` arrays to confirm attachment.

> ⚠️ **JWT Auth Filters:** MUST include `rules` with match patterns. A JWT filter without rules does NOT enforce authentication — it passes all traffic through unauthenticated. Always include rules like:
> ```json
> "rules": [{"match": {"prefix": "/api"}, "requires": {"type": "provider_name", "provider_name": "my-provider"}}]
> ```

### 6.3 Learn API from Traffic

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
  "route_pattern": "^/api/v1/.*",
  "target_sample_count": 50,
  "http_methods": ["GET", "POST", "PUT", "DELETE"]
}
```

**Step 4: Tell the user to generate traffic**

> ⚠️ **`ops_trace_request` is a DB diagnostic — it does NOT generate real HTTP traffic.**

After discovering the listener port and route paths, provide concrete curl examples:
```
Send traffic to http://localhost:{port}{path} — I need {target} samples.
Example: curl http://localhost:10016/api/v1/users
```

**Step 5: Monitor progress**
```
Tool: cp_get_learning_session
Args: { "id": "<session-id>" }
```
Report progress as `current_sample_count / target_sample_count (percentage)`. If samples aren't increasing, troubleshoot:
1. Is the backend service running?
2. Is the listener port correct?
3. Does the path match the `route_pattern` regex?

**Step 6: View discovered schemas**
```
Tool: cp_list_aggregated_schemas
```
Report: path, HTTP method, confidence score, sample count. Flag low-confidence schemas (below 0.5) as needing more traffic.

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
Args: { "schema_ids": ["<id-1>", "<id-2>"], "title": "My API", "version": "1.0.0" }
```

### 6.4 Blue/Green Deployment

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

### 6.5 Debug a Request

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
| Rate Limit Quota | `envoy.filters.http.rate_limit_quota` | Yes | Advanced quota management |
| CORS | `envoy.filters.http.cors` | Yes | Cross-origin resource sharing |
| Header Mutation | `envoy.filters.http.header_mutation` | Yes | Request/response header manipulation |
| Custom Response | `envoy.filters.http.custom_response` | Yes | Custom error responses |
| Health Check | `envoy.filters.http.health_check` | No | Health check endpoint responses |
| Credential Injector | `envoy.filters.http.credential_injector` | No | OAuth2/API key injection |
| Ext Proc | `envoy.filters.http.ext_proc` | No | External request/response processing |
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

This allows different rate limits, auth requirements, or processing modes per endpoint while keeping defaults at the listener level.

## 11. Common Pitfalls

| Pitfall | Details |
|---------|---------|
| **Router filter is auto-appended** | Don't add `envoy.filters.http.router` manually — Flowplane appends it automatically. Adding it yourself causes a validation error. |
| **Header/query matching not wired** | The API accepts header and query param matchers but they are **ignored in xDS generation**. Use path matchers only. |
| **Team from token** | Team context comes from the bearer token scopes or `?team=` param. Never hardcode team names in configs. |
| **Cluster must exist first** | A cluster must exist before you reference it in a route action. Create clusters before route configs. |
| **Filter attachment order** | Filters execute in the order they appear in the listener's filter chain. Auth filters should come before rate limiters. |
| **Route config must be bound** | Creating a route config doesn't automatically expose it. It must be bound to a listener (via `listener_route_configs`). |

## References

- [references/routing-cookbook.md](references/routing-cookbook.md) — Route patterns: forward, weighted, redirects, templates, per-route filters
- [references/mcp-tools.md](references/mcp-tools.md) — All MCP tool names, descriptions, and key parameters
- [references/filters-quick-ref.md](references/filters-quick-ref.md) — Filter config examples for common filters
