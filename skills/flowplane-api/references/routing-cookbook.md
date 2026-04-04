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

## Route Action Types

| Action | Fields | Description |
|--------|--------|-------------|
| `forward` | `cluster`, `timeoutSeconds`, `prefixRewrite`, `templateRewrite` | Forward to a single cluster |
| `weighted` | `totalWeight`, `clusters[]` (each with `name`, `weight`, optional `typedPerFilterConfig`) | Split traffic across clusters |
| `redirect` | `hostRedirect`, `pathRedirect`, `responseCode` | Return HTTP redirect |
