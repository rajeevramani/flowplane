# Routing Cookbook

Quick reference for common routing patterns. All examples are JSON payloads for route config creation via `cp_create_route_config` or the REST API.

## Basic Forward Route

Match a prefix and forward to a cluster:

```json
{
  "name": "basic-routes",
  "virtualHosts": [{
    "name": "default",
    "domains": ["*"],
    "routes": [{
      "name": "api",
      "match": { "path": { "type": "prefix", "value": "/api" } },
      "action": {
        "type": "forward",
        "cluster": "backend-cluster",
        "timeoutSeconds": 5,
        "prefixRewrite": "/internal/api"
      }
    }]
  }]
}
```

## Path Matchers

| Type | JSON | Use Case |
|------|------|----------|
| Exact | `{ "type": "exact", "value": "/health" }` | Health probes, single files |
| Prefix | `{ "type": "prefix", "value": "/api" }` | API namespaces |
| Regex | `{ "type": "regex", "value": "^/v[0-9]+/" }` | Version guards |
| Template | `{ "type": "template", "template": "/users/{user_id}" }` | RESTful resource paths |

## Weighted (Blue/Green) Routing

Split traffic across clusters with optional per-cluster filter overrides:

```json
{
  "name": "blue-green",
  "match": { "path": { "type": "prefix", "value": "/" } },
  "action": {
    "type": "weighted",
    "totalWeight": 100,
    "clusters": [
      {
        "name": "app-blue",
        "weight": 80,
        "typedPerFilterConfig": {
          "envoy.filters.http.local_ratelimit": {
            "stat_prefix": "blue_limit",
            "token_bucket": { "max_tokens": 40, "tokens_per_fill": 40, "fill_interval_ms": 1000 }
          }
        }
      },
      { "name": "app-green", "weight": 20 }
    ]
  }
}
```

## Redirects

Return redirects without touching upstreams:

```json
{
  "name": "docs-redirect",
  "match": { "path": { "type": "prefix", "value": "/docs" } },
  "action": {
    "type": "redirect",
    "hostRedirect": "docs.new.example.com",
    "pathRedirect": "/",
    "responseCode": 302
  }
}
```

## Template Match with Rewrite

Capture path parameters and reshape before forwarding:

```json
{
  "name": "user-profile",
  "match": {
    "path": { "type": "template", "template": "/api/v1/users/{user_id}" }
  },
  "action": {
    "type": "forward",
    "cluster": "users-internal",
    "templateRewrite": "/internal/{user_id}/profile"
  }
}
```

## Per-Route Filter Overrides

Apply filter config at the route level using `typedPerFilterConfig`:

```json
{
  "name": "public-endpoint",
  "match": { "path": { "type": "prefix", "value": "/public" } },
  "action": { "type": "forward", "cluster": "public-backend" },
  "typedPerFilterConfig": {
    "envoy.filters.http.local_ratelimit": {
      "stat_prefix": "public_api",
      "token_bucket": { "max_tokens": 100, "tokens_per_fill": 100, "fill_interval_ms": 1000 }
    },
    "envoy.filters.http.jwt_authn": {
      "requirement_name": "allow_missing"
    }
  }
}
```

Overrides can also be placed on **virtual hosts** (apply to all routes in that host) or **weighted clusters** (apply per-backend).

## Exempt a Sub-Path from a Filter

Disable a filter for a specific sub-path while keeping it active on the parent prefix. The exempt route must come **before** the broader route (Envoy uses first-match):

```json
{
  "name": "<route-config-name>",
  "virtualHosts": [{
    "name": "<vhost-name>",
    "domains": ["*"],
    "routes": [
      {
        "name": "<exempt-route>",
        "match": { "path": { "type": "prefix", "value": "<specific-sub-path>" } },
        "action": { "type": "forward", "cluster": "<cluster>" },
        "typedPerFilterConfig": {
          "<envoy-filter-name>": { "disabled": true }
        }
      },
      {
        "name": "<protected-route>",
        "match": { "path": { "type": "prefix", "value": "<broad-prefix>" } },
        "action": { "type": "forward", "cluster": "<cluster>" }
      }
    ]
  }]
}
```

**Route order matters:** The more-specific exempt route MUST come before the broader prefix. Envoy uses first-match — if reversed, the broad prefix matches everything.

**Updating existing routes:** `cp_update_route_config` replaces the entire `virtualHosts` array. Fetch existing config with `cp_get_route_config` first, then include all existing routes plus the new exempt route in the correct position.

## Retry Policy

Configure retries on route actions to handle transient failures:

```json
{
  "name": "resilient-route",
  "match": { "path": { "type": "prefix", "value": "/api" } },
  "action": {
    "type": "forward",
    "cluster": "my-backend",
    "timeoutSeconds": 30,
    "retryPolicy": {
      "maxRetries": 3,
      "retryOn": ["5xx", "reset", "connect-failure", "retriable-4xx"],
      "perTryTimeoutSeconds": 10,
      "backoff": {
        "baseIntervalMs": 100,
        "maxIntervalMs": 1000
      }
    }
  }
}
```

- `retryOn` — conditions that trigger a retry: `5xx`, `reset`, `connect-failure`, `retriable-4xx`, `gateway-error`, `refused-stream`
- `perTryTimeoutSeconds` — timeout for each individual attempt (must be less than `timeoutSeconds`)
- `backoff` — exponential backoff between retries

## Circuit Breakers

Circuit breakers are configured at the **cluster** level (not routes). They limit connection and request counts to prevent cascade failures:

```json
{
  "name": "protected-backend",
  "serviceName": "my-service",
  "endpoints": [{ "host": "backend-svc", "port": 8000 }],
  "lbPolicy": "ROUND_ROBIN",
  "circuitBreakers": {
    "default": {
      "maxConnections": 10,
      "maxPendingRequests": 5,
      "maxRequests": 20,
      "maxRetries": 2
    }
  }
}
```

When limits are hit, Envoy returns `503 Service Unavailable` instead of queuing more requests.

## Route Action Types

| Action | Fields | Description |
|--------|--------|-------------|
| `forward` | `cluster`, `timeoutSeconds`, `prefixRewrite`, `templateRewrite`, `retryPolicy` | Forward to a single cluster |
| `weighted` | `totalWeight`, `clusters[]` (each with `name`, `weight`, optional `typedPerFilterConfig`) | Split traffic across clusters |
| `redirect` | `hostRedirect`, `pathRedirect`, `responseCode` | Return HTTP redirect |
