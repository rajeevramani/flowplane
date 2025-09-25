# Routing Cookbook

This guide expands on the basic route walkthrough in [getting-started](getting-started.md) and shows how to express the different route actions, matchers, and per-route filter overrides supported by Flowplane. Each example is a JSON payload you can POST to `/api/v1/routes`. Field names follow the conventions used throughout the API: camelCase for control-plane structures (e.g., `virtualHosts`, `typedPerFilterConfig`) and snake_case inside filter blocks (e.g., `token_bucket`).

> Tip: Open the live API reference at `http://127.0.0.1:8080/swagger-ui` to inspect the schema and try these calls directly.

## 1. Basic Forward Route
The simplest route matches a URL prefix and forwards traffic to a single cluster.

```bash
curl -sS -X POST http://127.0.0.1:8080/api/v1/routes \
  -H 'Content-Type: application/json' \
  -d '{
    "name": "basic-routes",
    "virtualHosts": [
      {
        "name": "default",
        "domains": ["*"],
        "routes": [
          {
            "name": "api",
            "match": { "path": { "type": "prefix", "value": "/api" } },
            "action": {
              "type": "forward",
              "cluster": "backend-cluster",
              "timeoutSeconds": 5,
              "prefixRewrite": "/internal/api"
            }
          }
        ]
      }
    ]
  }'
```

### Supported Path Matchers
| Matcher | JSON | Ideal for |
| ------- | ---- | --------- |
| Exact   | `{ "type": "exact", "value": "/health" }` | Health probes (`/ready`, `/live`), single-file assets (`/robots.txt`), feature toggles that should never fall back to broader handlers. |
| Prefix  | `{ "type": "prefix", "value": "/api" }` | API namespaces (`/api/`, `/internal/`), tenant or product partitions, and pairing with weighted routes for progressive roll-outs under a shared prefix. |
| Regex   | `{ "type": "regex", "value": "^/v[0-9]+/" }` | Version guards (`/v1`, `/v2`), matching optional segments (`^/reports/(daily|weekly)/`), or catching legacy URL patterns with minimal new routes. |
| Template| `{ "type": "template", "template": "/users/{user_id}" }` | Resource-centric paths where you may need to capture identifiers for rewrites (`/accounts/{id}/users/{user_id}`) or propagate parameters to upstream services. |

Support for refining matches with headers and query parameters is planned. Those fields are accepted by the API today but are ignored when the control plane renders Envoy resources, so stick to path-based matching until the remaining wiring is in place.

## 2. Weighted (Blue/Green) Routing
Distribute traffic across multiple clusters with weights, and optionally override filters per cluster.

```bash
curl -sS -X POST http://127.0.0.1:8080/api/v1/routes \
  -H 'Content-Type: application/json' \
  -d '{
    "name": "weighted-routes",
    "virtualHosts": [
      {
        "name": "default",
        "domains": ["app.example.com"],
        "routes": [
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
                      "token_bucket": {
                        "max_tokens": 40,
                        "tokens_per_fill": 40,
                        "fill_interval_ms": 1000
                      }
                    }
                  }
                },
                {
                  "name": "app-green",
                  "weight": 20,
                  "typedPerFilterConfig": {
                    "envoy.filters.http.jwt_authn": {
                      "requirement_name": "allow_optional"
                    }
                  }
                }
              ]
            }
          }
        ]
      }
    ]
  }'
```

Use this pattern for gradual roll-outs, A/B testing, or tenant segmentation. Each weighted cluster can attach its own scoped filter configuration (`typedPerFilterConfig`).

## 3. Redirects and Rewrites
Envoy can return redirects without touching your upstreams.

```bash
curl -sS -X POST http://127.0.0.1:8080/api/v1/routes \
  -H 'Content-Type: application/json' \
  -d '{
    "name": "redirect-routes",
    "virtualHosts": [
      {
        "name": "legacy",
        "domains": ["legacy.example.com"],
        "routes": [
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
        ]
      }
    ]
  }'
```

For trailing slash fixes or vanity URLs, use exact matches and adjust the `pathRedirect` accordingly.

## 4. Template Matches & Rewrites
Use URI templates when you want to capture path parameters and optionally rewrite them before sending traffic upstream.

### Downstream and Upstream Paths Match
When the downstream and upstream shapes are identical, a template match keeps Envoy aware of the parameters without rewriting:

```json
{
  "name": "user-template",
  "match": {
    "path": {
      "type": "template",
      "template": "/api/v1/users/{user_id}"
    }
  },
  "action": {
    "type": "forward",
    "cluster": "users-cluster"
  }
}
```

### Downstream Path Differs From Upstream Path
Combine a template matcher with `templateRewrite` to reshape the request before forwarding. This example maps public `/api/v1/users/{user_id}` calls to an internal `/internal/{user_id}/profile` endpoint:

```json
{
  "name": "user-profile",
  "match": {
    "path": {
      "type": "template",
      "template": "/api/v1/users/{user_id}"
    }
  },
  "action": {
    "type": "forward",
    "cluster": "users-internal",
    "templateRewrite": "/internal/{user_id}/profile"
  }
}
```

For simpler rewrites (no parameters), use `prefixRewrite` to replace the prefix segment, or `pathRedirect` (see above) when you want Envoy to issue a redirect instead of forwarding.

## 5. Scoped Filters on Routes
Any route, virtual host, or weighted cluster can attach additional HTTP filter configuration through `typedPerFilterConfig`. The keys are Envoy filter names; the values are the structured configs defined in this repo.

### Per-Route Local Rate Limit
```json
"typedPerFilterConfig": {
  "envoy.filters.http.local_ratelimit": {
    "stat_prefix": "per_route",
    "token_bucket": {
      "max_tokens": 10,
      "tokens_per_fill": 10,
      "fill_interval_ms": 1000
    }
  }
}
```

### JWT Requirement Override
```json
"typedPerFilterConfig": {
  "envoy.filters.http.jwt_authn": {
    "requirement_name": "allow_missing"
  }
}
```

Combine overrides to tailor auth or throttling policies per endpoint while keeping defaults at the listener level.

## 6. Updating and Inspecting Routes
- List routes: `GET /api/v1/routes`
- Fetch a specific route: `GET /api/v1/routes/{name}`
- Update: `PUT /api/v1/routes/{name}` with the same payload shape
- Delete: `DELETE /api/v1/routes/{name}`

All operations are documented in the Swagger UI. Payloads are validated before persistence; errors return HTTP 400 with descriptive messages.

## 7. Putting It Together
A common workflow:

1. Create a base route definition with forward actions.
2. Add scoped rate limits for hot endpoints via `typedPerFilterConfig`.
3. Introduce a weighted route when testing a new backend version.
4. Use redirects for legacy paths once traffic has migrated.

Mix and match these techniques to evolve your gateway without redeploying Envoy. For a full walkthrough (clusters → routes → listeners), see [getting-started](getting-started.md); for filter configuration details, see [filters](filters.md).
