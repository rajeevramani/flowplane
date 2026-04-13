# Filters Quick Reference

## All Filters

| Filter | Envoy Name | Type Key | Per-Route | Status |
|--------|-----------|----------|-----------|--------|
| OAuth2 | `envoy.filters.http.oauth2` | `oauth2` | No | Implemented |
| JWT Auth | `envoy.filters.http.jwt_authn` | `jwt_auth` | Yes (reference_only) | Implemented |
| Ext Auth | `envoy.filters.http.ext_authz` | `ext_authz` | Yes (full_config) | Implemented |
| RBAC | `envoy.filters.http.rbac` | `rbac` | Yes (full_config) | Implemented |
| Local Rate Limit | `envoy.filters.http.local_ratelimit` | `local_rate_limit` | Yes (full_config) | Implemented |
| Rate Limit | `envoy.filters.http.ratelimit` | `rate_limit` | Yes | **Not implemented** (400 error) |
| CORS | `envoy.filters.http.cors` | `cors` | Yes (disable only) | Implemented |
| Header Mutation | `envoy.filters.http.header_mutation` | `header_mutation` | Yes (full_config) | Implemented |
| Custom Response | `envoy.filters.http.custom_response` | `custom_response` | Yes (disable only) | Implemented |
| Compressor | `envoy.filters.http.compressor` | `compressor` | Yes (disable only) | Implemented (Envoy NACK bug on attach) |
| MCP | `envoy.filters.http.mcp` | `mcp` | Yes (disable only) | Implemented |
| Router | `envoy.filters.http.router` | N/A | N/A | **Auto-appended** — do not add manually |

**NOT FilterTypes** (XDS modules exist but not registered in `src/domain/filter.rs` FilterType enum):
`credential_injector`, `ext_proc`, `health_check`, `rate_limit_quota`, `wasm`

## Common Filter Configs

All filter create requests use this structure:
```json
{
  "name": "filter-name",
  "filterType": "type_key",
  "config": {
    "type": "type_key",
    "config": { ... }
  }
}
```

### Local Rate Limit

```json
{
  "name": "my-rate-limit",
  "filterType": "local_rate_limit",
  "config": {
    "type": "local_rate_limit",
    "config": {
      "stat_prefix": "local_rate_limit",
      "token_bucket": {
        "max_tokens": 10,
        "tokens_per_fill": 10,
        "fill_interval_ms": 60000
      },
      "status_code": 429,
      "filter_enabled": { "numerator": 100, "denominator": "hundred" },
      "filter_enforced": { "numerator": 100, "denominator": "hundred" }
    }
  }
}
```

Denominator values are snake_case: `hundred`, `ten_thousand`, `million`.

### JWT Auth

```json
{
  "name": "my-jwt-auth",
  "filterType": "jwt_auth",
  "config": {
    "type": "jwt_auth",
    "config": {
      "providers": {
        "my-provider": {
          "issuer": "https://auth.example.com",
          "audiences": ["my-api"],
          "jwks": {
            "type": "remote",
            "http_uri": {
              "uri": "https://auth.example.com/.well-known/jwks.json",
              "cluster": "auth-jwks-cluster",
              "timeout_ms": 5000
            },
            "cache_duration_seconds": 300
          },
          "fromHeaders": [{ "name": "Authorization", "value_prefix": "Bearer " }],
          "forward": true
        }
      },
      "bypass_cors_preflight": true
    }
  }
}
```

**Rules** (without these, JWT is NOT enforced on any path):
```yaml
rules:
  - match:
      path:
        Prefix: "/"          # capital P — PathMatch variants are Prefix/Exact/Regex/Template
    requires:
      type: "provider_name"   # tagged enum — always include "type"
      provider_name: "auth0"
```
Rules missing a `match` clause cause Envoy to NACK the listener (LDS rejection) — the active listener silently stays on the old config.

Local JWKS requires valid public keys — empty `{"keys":[]}` causes Envoy NACK.

### CORS

```json
{
  "name": "my-cors",
  "filterType": "cors",
  "config": {
    "type": "cors",
    "config": {
      "policy": {
        "allow_origin": [
          { "type": "exact", "value": "https://example.com" },
          { "type": "prefix", "value": "https://app." }
        ],
        "allow_methods": ["GET", "POST", "PUT", "DELETE", "OPTIONS"],
        "allow_headers": ["authorization", "content-type", "x-request-id"],
        "expose_headers": ["x-custom-header"],
        "max_age": 86400,
        "allow_credentials": true
      }
    }
  }
}
```

Origin matcher types: `exact`, `prefix`, `suffix`, `contains`, `regex`.
Config has extra nesting: `config.config.policy` wraps the actual policy fields.

### Header Mutation

```json
{
  "name": "my-header-mutation",
  "filterType": "header_mutation",
  "config": {
    "type": "header_mutation",
    "config": {
      "request_headers_to_add": [
        { "key": "X-Request-Id", "value": "test-123", "append": false }
      ],
      "request_headers_to_remove": [],
      "response_headers_to_add": [
        { "key": "X-Content-Type-Options", "value": "nosniff", "append": false },
        { "key": "X-Frame-Options", "value": "DENY", "append": false }
      ],
      "response_headers_to_remove": ["X-Powered-By"]
    }
  }
}
```

### Compressor

```json
{
  "name": "my-compressor",
  "filterType": "compressor",
  "config": {
    "type": "compressor",
    "config": {
      "response_direction_config": {
        "common_config": {
          "min_content_length": 100,
          "content_type": ["application/json", "text/html", "text/plain"],
          "disable_on_etag_header": false,
          "remove_accept_encoding_header": false
        }
      },
      "compressor_library": {
        "type": "gzip",
        "compression_level": "best_speed",
        "compression_strategy": "default_strategy",
        "memory_level": 5,
        "window_bits": 12,
        "chunk_size": 4096
      }
    }
  }
}
```

**Known bug:** Envoy NACKs on attach due to empty `RuntimeFeatureFlag.runtime_key` in `compressor.rs:to_any()`.

### Ext Authz

```json
{
  "name": "my-ext-authz",
  "filterType": "ext_authz",
  "config": {
    "type": "ext_authz",
    "config": {
      "service": {
        "type": "http",
        "server_uri": {
          "uri": "http://authz-service-cluster",
          "cluster": "authz-service-cluster",
          "timeout_ms": 500
        },
        "path_prefix": "/auth",
        "authorization_request": {
          "allowed_headers": ["authorization", "x-request-id"],
          "headers_to_add": []
        }
      },
      "failure_mode_allow": false,
      "clear_route_cache": false,
      "stat_prefix": "ext_authz",
      "status_on_error": 403,
      "include_peer_certificate": false
    }
  }
}
```

gRPC mode `target_uri` is a cluster name, not a URL.

### Custom Response

```json
{
  "name": "my-custom-response",
  "filterType": "custom_response",
  "config": {
    "type": "custom_response",
    "config": {
      "matchers": [
        {
          "status_code": { "type": "range", "min": 500, "max": 599 },
          "response": {
            "status_code": 500,
            "body": "{\"error\": \"Internal server error\"}",
            "headers": { "content-type": "application/json" }
          }
        }
      ]
    }
  }
}
```

Status code matcher: `{"type": "exact", "code": 503}` or `{"type": "range", "min": 500, "max": 599}`.

### OAuth2

**Prerequisites — create these BEFORE the filter, or Envoy NACKs the listener:**

1. **IDP cluster** (with `useTls: true`) for `token_endpoint.cluster`
2. **Token secret** — your IDP client secret as a `generic_secret`
3. **HMAC secret** — random 32-byte signing key for OAuth2 cookies:
   ```bash
   HMAC=$(openssl rand -base64 32)
   flowplane secret create --name oauth-hmac --type generic_secret \
     --config "{\"type\":\"generic_secret\",\"secret\":\"$HMAC\"}"
   ```

```json
{
  "name": "my-oauth2",
  "filterType": "oauth2",
  "config": {
    "type": "oauth2",
    "config": {
      "token_endpoint": {
        "uri": "https://auth.example.com/oauth/token",
        "cluster": "oauth2-auth-cluster",
        "timeout_ms": 5000
      },
      "authorization_endpoint": "https://auth.example.com/authorize",
      "credentials": {
        "client_id": "my-app",
        "token_secret": { "name": "oauth2-client-secret" },
        "hmac_secret": { "name": "oauth-hmac" }
      },
      "redirect_uri": "https://app.example.com/callback",
      "redirect_path": "/callback",
      "signout_path": "/logout",
      "auth_scopes": ["openid", "profile", "email"],
      "auth_type": "url_encoded_body",
      "forward_bearer_token": true,
      "use_refresh_token": true,
      "default_expires_in_seconds": 3600,
      "stat_prefix": "oauth2_filter"
    }
  }
}
```

**Critical shape details:**
- `token_secret` is `{ "name": "..." }` — no `type` field (despite older docs)
- `hmac_secret` is **required** since v0.2.3 — referenced like `token_secret`
- `signout_path` is **required** by Envoy proto validation, even if you don't expose a signout flow — set it to `/signout` if unsure
- OAuth2 is **listener-only** — per-route config is not supported. Use `pass_through_matcher` to bypass paths like `/healthz`

## REST API Operations

### Create a filter

```
POST /api/v1/filters
{
  "name": "filter-name",
  "filterType": "type_key",
  "config": {
    "type": "type_key",
    "config": { ... }
  }
}
```

### Install filter on a listener

```
POST /api/v1/filters/{filterId}/installations
{
  "listenerName": "my-listener",
  "order": 1
}
```

### Attach filter to a route config

```
POST /api/v1/filters/{filterId}/configurations
{
  "scopeType": "route-config",
  "scopeId": "my-route-config"
}
```

### Per-route override

```
POST /api/v1/filters/{filterId}/configurations
{
  "scopeType": "route",
  "scopeId": "route-config-name/vhost-name/route-name",
  "settings": {
    "behavior": "override",
    "config": { ... filter-specific override config ... }
  }
}
```

### Disable filter on a route

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

> Use `behavior: "disable"` to fully exempt a route (e.g. `/health`, `/public`). Use `behavior: "override"` when you want to apply different filter settings per route.
