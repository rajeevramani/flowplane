# Filters

Add rate limiting, authentication, CORS, and more to your gateway. Filters are Envoy HTTP filter configurations managed through Flowplane's control plane — you define them once and attach them to listeners or route configs.

## Filter Types

| Filter | Type Key | Envoy Filter | Per-Route | Description |
|--------|----------|-------------|-----------|-------------|
| OAuth2 | `oauth2` | `envoy.filters.http.oauth2` | No | OAuth 2.0 authorization code flow |
| JWT Auth | `jwt_auth` | `envoy.filters.http.jwt_authn` | Yes | Validate JWTs against JWKS endpoints |
| Ext Auth | `ext_authz` | `envoy.filters.http.ext_authz` | Yes | Delegate auth decisions to an external service |
| RBAC | `rbac` | `envoy.filters.http.rbac` | Yes | Role-based access control policies |
| Local Rate Limit | `local_rate_limit` | `envoy.filters.http.local_ratelimit` | Yes | Per-instance token bucket rate limiting |
| Rate Limit | `rate_limit` | `envoy.filters.http.ratelimit` | Yes | Distributed rate limiting via external service |
| Rate Limit Quota | `rate_limit_quota` | `envoy.filters.http.rate_limit_quota` | Yes | Quota-based rate limiting |
| CORS | `cors` | `envoy.filters.http.cors` | Yes | Cross-origin resource sharing headers |
| Header Mutation | `header_mutation` | `envoy.filters.http.header_mutation` | Yes | Add, remove, or modify request/response headers |
| Custom Response | `custom_response` | `envoy.filters.http.custom_response` | Yes | Override responses for specific conditions |
| Health Check | `health_check` | `envoy.filters.http.health_check` | No | Respond to health check probes at the proxy |
| Credential Injector | `credential_injector` | `envoy.filters.http.credential_injector` | No | Inject credentials into upstream requests |
| Ext Proc | `ext_proc` | `envoy.filters.http.ext_proc` | No | External processing via gRPC service |
| Compressor | `compressor` | `envoy.filters.http.compressor` | Yes | Compress response bodies (gzip, brotli, etc.) |

**Per-Route** indicates whether the filter supports per-route overrides via `typedPerFilterConfig` (see [Per-Route Overrides](#per-route-overrides)).

> The Router filter is always auto-appended by Flowplane as the final filter in the chain. Never add it manually.

## Workflow: Create, Attach, Verify

Every filter follows a three-step workflow: create the filter definition, attach it to a listener (or route config), then verify it's active.

### CLI

```bash
# 1. Create the filter from a JSON file
flowplane filter create -f filter.json

# 2. Attach to a listener with an execution order
flowplane filter attach <filter-name> --listener <listener-name> --order <n>

# 3. Verify the attachment
flowplane filter get <filter-name>
# Look for listenerInstallations in the output
```

### MCP

```
# 1. Create the filter
cp_create_filter {
  "name": "my-filter",
  "filterType": "local_rate_limit",
  "config": { "type": "...", "config": { ... } }
}

# 2. Attach to a listener
cp_attach_filter {
  "filter": "my-filter",
  "listener": "my-listener",
  "order": 10
}

# 3. Verify
cp_get_filter { "name": "my-filter" }
# Check listenerInstallations in the response
```

See [CLI Reference](cli-reference.md) for the full list of filter commands and flags.

## Attachment Levels

Filters can be attached at two levels, controlling how broadly they apply.

### Listener-level

Attaches the filter to all traffic arriving on that listener's port.

```bash
# CLI
flowplane filter attach my-filter --listener my-listener --order 10
```

```
# MCP
cp_attach_filter { "filter": "my-filter", "listener": "my-listener", "order": 10 }
```

### Route-config-level

Attaches the filter to all routes within a specific route configuration. Available via MCP only.

```
# MCP
cp_attach_filter { "filter": "my-filter", "route_config": "my-route-config", "order": 10 }
```

Use listener-level when the filter should apply to everything on that port (e.g., rate limiting all inbound traffic). Use route-config-level when you want the filter scoped to a specific set of routes.

## Per-Route Overrides

Filters marked **Per-Route: Yes** in the table above can be customized, overridden, or disabled on individual routes or virtual hosts using `typedPerFilterConfig`.

### Override config for a route

Add `typedPerFilterConfig` to a route or virtual host in your route config JSON, keyed by the Envoy filter name:

```json
{
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
}
```

This overrides the listener-level rate limit configuration for that specific route.

### Disable a filter for a route

To skip a filter entirely on a specific route, set `disabled: true`:

```json
{
  "typedPerFilterConfig": {
    "envoy.filters.http.jwt_authn": {
      "disabled": true
    }
  }
}
```

> **`disabled` vs `allow_missing`**: `disabled: true` skips the filter entirely — it never executes. `allow_missing` still runs the filter but won't reject requests that are missing tokens. Use `disabled` for public endpoints; use `allow_missing` for endpoints that optionally accept tokens.

### Exempt a sub-path

Combine per-route overrides with route matching to exempt specific paths. For example, disable JWT auth on a health check path while keeping it active on all other routes:

```json
{
  "match": { "prefix": "/healthz" },
  "route": { "cluster": "my-service" },
  "typedPerFilterConfig": {
    "envoy.filters.http.jwt_authn": { "disabled": true }
  }
}
```

## Execution Order

The `--order` value (CLI) or `"order"` field (MCP) controls where the filter sits in the chain. Lower numbers execute first. The recommended order:

| Order | Filter | Reason |
|-------|--------|--------|
| 1 | CORS | Handle preflight before any auth |
| 2 | JWT Auth | Validate tokens early |
| 3 | Ext Auth | External authorization decisions |
| 4 | RBAC | Enforce access policies |
| 5 | Rate Limit | Don't waste tokens on unauthenticated requests |
| 6 | Header Mutation | Modify headers for upstream |
| — | Router | Auto-appended by Flowplane |

> Order values must be unique per listener. If you attach two filters with the same order to one listener, the second attachment will fail.

## Examples

These examples assume you have a running Flowplane instance with an httpbin service available. See [Getting Started](getting-started.md) for setup instructions.

### 1. Rate Limiting

Limit requests to 3 per minute using a local token bucket.

**Create the filter definition** (`/tmp/rl.json`):

```json
{
  "name": "demo-rate-limit",
  "filterType": "local_rate_limit",
  "config": {
    "type": "local_rate_limit",
    "config": {
      "stat_prefix": "demo_rl",
      "token_bucket": {
        "max_tokens": 3,
        "tokens_per_fill": 3,
        "fill_interval_ms": 60000
      }
    }
  }
}
```

**CLI workflow:**

```bash
# Expose a service first (creates cluster, route config, and listener)
flowplane expose http://httpbin:80 --name demo

# Create the filter
flowplane filter create -f /tmp/rl.json

# Attach to the listener
flowplane filter attach demo-rate-limit --listener demo-listener --order 1

# Verify attachment
flowplane filter get demo-rate-limit

# Test — first 3 requests succeed, rest are rate limited
# Use the port from `flowplane expose` output (10001 on a fresh stack)
PORT=10001
for i in $(seq 1 5); do
  echo "Request $i: $(curl -s -o /dev/null -w '%{http_code}' http://localhost:$PORT/get)"
done
# Expected output:
# Request 1: 200
# Request 2: 200
# Request 3: 200
# Request 4: 429
# Request 5: 429
```

**MCP equivalent:**

```
# Create the filter
cp_create_filter {
  "name": "demo-rate-limit",
  "filterType": "local_rate_limit",
  "config": {
    "type": "local_rate_limit",
    "config": {
      "stat_prefix": "demo_rl",
      "token_bucket": {
        "max_tokens": 3,
        "tokens_per_fill": 3,
        "fill_interval_ms": 60000
      }
    }
  }
}

# Attach to listener
cp_attach_filter {
  "filter": "demo-rate-limit",
  "listener": "demo-listener",
  "order": 1
}

# Verify
cp_get_filter { "name": "demo-rate-limit" }
```

### 2. JWT Authentication

Protect `/api` routes with JWT validation against an external JWKS endpoint.

**Create the filter definition** (`/tmp/jwt.json`):

```json
{
  "name": "demo-jwt",
  "filterType": "jwt_auth",
  "config": {
    "type": "jwt_auth",
    "config": {
      "providers": {
        "test-provider": {
          "issuer": "https://auth.example.com",
          "audiences": ["demo-api"],
          "jwks": {
            "type": "remote",
            "http_uri": {
              "uri": "https://auth.example.com/.well-known/jwks.json",
              "cluster": "auth-cluster",
              "timeout_ms": 5000
            }
          },
          "forward": true
        }
      },
      "rules": [
        {
          "match": { "path": { "Prefix": "/api" } },
          "requires": {
            "type": "provider_name",
            "provider_name": "test-provider"
          }
        }
      ]
    }
  }
}
```

> **Gotcha**: A JWT filter without `rules` does NOT enforce authentication. All traffic passes through unauthenticated. Always define at least one rule.

**CLI workflow:**

```bash
# Create the filter
flowplane filter create -f /tmp/jwt.json

# Attach to the listener (after CORS if present)
flowplane filter attach demo-jwt --listener demo-listener --order 2

# Verify attachment
flowplane filter get demo-jwt

# Test — request without a token is rejected
curl -s -o /dev/null -w '%{http_code}' http://localhost:$PORT/api/data
# Expected: 401

# Request with a valid token succeeds
curl -s -o /dev/null -w '%{http_code}' \
  -H "Authorization: Bearer <valid-jwt>" \
  http://localhost:$PORT/api/data
# Expected: 200
```

**MCP equivalent:**

```
# Create the filter
cp_create_filter {
  "name": "demo-jwt",
  "filterType": "jwt_auth",
  "config": {
    "type": "jwt_auth",
    "config": {
      "providers": {
        "test-provider": {
          "issuer": "https://auth.example.com",
          "audiences": ["demo-api"],
          "jwks": {
            "type": "remote",
            "http_uri": {
              "uri": "https://auth.example.com/.well-known/jwks.json",
              "cluster": "auth-cluster",
              "timeout_ms": 5000
            }
          },
          "forward": true
        }
      },
      "rules": [
        {
          "match": { "path": { "Prefix": "/api" } },
          "requires": { "type": "provider_name", "provider_name": "test-provider" }
        }
      ]
    }
  }
}

# Attach
cp_attach_filter { "filter": "demo-jwt", "listener": "demo-listener", "order": 2 }

# Verify
cp_get_filter { "name": "demo-jwt" }
```

### 3. CORS

Allow cross-origin requests from a specific frontend domain.

**Create the filter definition** (`/tmp/cors.json`):

```json
{
  "name": "demo-cors",
  "filterType": "cors",
  "config": {
    "type": "cors",
    "config": {
      "policy": {
        "allow_origin": [
          { "type": "exact", "value": "https://app.example.com" }
        ],
        "allow_methods": ["GET", "POST", "PUT", "DELETE", "OPTIONS"],
        "allow_headers": ["Authorization", "Content-Type"],
        "max_age": 86400,
        "allow_credentials": true
      }
    }
  }
}
```

**CLI workflow:**

```bash
# Create the filter
flowplane filter create -f /tmp/cors.json

# Attach as the first filter (CORS should run before auth)
flowplane filter attach demo-cors --listener demo-listener --order 1

# Verify attachment
flowplane filter get demo-cors

# Test preflight request
curl -s -D - -o /dev/null \
  -H "Origin: https://app.example.com" \
  -H "Access-Control-Request-Method: POST" \
  -X OPTIONS \
  http://localhost:$PORT/api/data
# Expected headers in response:
# access-control-allow-origin: https://app.example.com
# access-control-allow-methods: GET, POST, PUT, DELETE
# access-control-allow-headers: Authorization, Content-Type
# access-control-max-age: 86400

# Test actual request with allowed origin
curl -s -D - -o /dev/null \
  -H "Origin: https://app.example.com" \
  http://localhost:$PORT/get
# Expected: access-control-allow-origin header present

# Test with disallowed origin
curl -s -D - -o /dev/null \
  -H "Origin: https://evil.example.com" \
  http://localhost:$PORT/get
# Expected: no access-control-allow-origin header from Envoy
```

**MCP equivalent:**

```
# Create the filter
cp_create_filter {
  "name": "demo-cors",
  "filterType": "cors",
  "config": {
    "type": "cors",
    "config": {
      "policy": {
        "allow_origin": [
          { "type": "exact", "value": "https://app.example.com" }
        ],
        "allow_methods": ["GET", "POST", "PUT", "DELETE", "OPTIONS"],
        "allow_headers": ["Authorization", "Content-Type"],
        "max_age": 86400,
        "allow_credentials": true
      }
    }
  }
}

# Attach as first filter
cp_attach_filter { "filter": "demo-cors", "listener": "demo-listener", "order": 1 }

# Verify
cp_get_filter { "name": "demo-cors" }
```

## Gotchas

> **Router is auto-appended.** Flowplane always adds the Router filter as the last filter in the chain. Never create or attach a Router filter manually — it will cause errors or duplicate routing.

> **JWT without rules = no enforcement.** If you create a `jwt_auth` filter without any `rules`, the filter is present but never requires a token. Every request passes through unauthenticated. Always include at least one rule with a `requires` clause.

> **`filter_enabled` defaults.** Envoy's `filter_enabled` field defaults to 0% for some filter types, meaning the filter is present but never executes. Flowplane handles this by setting sensible defaults, but if you're passing raw Envoy config, verify `filter_enabled` is set to 100% (or omitted to use Flowplane's default).

> **Order values must be unique per listener.** Two filters cannot share the same order value on a single listener. Choose distinct integers and leave gaps (e.g., 10, 20, 30) so you can insert filters later without reordering.

---

See also: [Getting Started](getting-started.md) | [CLI Reference](cli-reference.md)
