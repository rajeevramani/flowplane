# Filters Quick Reference

## All Filters

| Filter | Envoy Name | Per-Route | Notes |
|--------|-----------|-----------|-------|
| OAuth2 | `envoy.filters.http.oauth2` | No | Full authorization code flow with PKCE |
| JWT Auth | `envoy.filters.http.jwt_authn` | Yes | JWT validation with remote/local JWKS |
| Ext Auth | `envoy.filters.http.ext_authz` | Yes | Delegate auth to external service |
| RBAC | `envoy.filters.http.rbac` | Yes | Policy-based access control |
| Local Rate Limit | `envoy.filters.http.local_ratelimit` | Yes | Per-instance token bucket |
| Rate Limit | `envoy.filters.http.ratelimit` | Yes | Distributed rate limiting |
| Rate Limit Quota | `envoy.filters.http.rate_limit_quota` | Yes | Dynamic quota management (RLQS) |
| CORS | `envoy.filters.http.cors` | Yes | Cross-origin resource sharing |
| Header Mutation | `envoy.filters.http.header_mutation` | Yes | Add/remove/modify headers |
| Custom Response | `envoy.filters.http.custom_response` | Yes | Custom error responses by status code |
| Health Check | `envoy.filters.http.health_check` | No | Respond to health probes |
| Credential Injector | `envoy.filters.http.credential_injector` | No | Inject OAuth2/API key credentials |
| Ext Proc | `envoy.filters.http.ext_proc` | No | External request/response processing |
| Compressor | `envoy.filters.http.compressor` | Yes | Response compression (gzip, brotli) |
| Router | `envoy.filters.http.router` | N/A | **Auto-appended** — do not add manually |

## Common Filter Configs

### Local Rate Limit

Listener-level (global):
```json
{
  "name": "envoy.filters.http.local_ratelimit",
  "filter": {
    "type": "local_rate_limit",
    "stat_prefix": "global_rl",
    "token_bucket": {
      "max_tokens": 1000,
      "tokens_per_fill": 1000,
      "fill_interval_ms": 1000
    }
  }
}
```

Per-route override:
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

### JWT Authentication

Listener-level:
```json
{
  "name": "envoy.filters.http.jwt_authn",
  "filter": {
    "type": "jwt_authn",
    "providers": {
      "my_provider": {
        "issuer": "https://auth.example.com",
        "audiences": ["my-api"],
        "remote_jwks": {
          "http_uri": {
            "uri": "https://auth.example.com/.well-known/jwks.json",
            "cluster": "auth-cluster",
            "timeout_seconds": 5
          }
        },
        "forward": true
      }
    },
    "rules": [{
      "match": { "prefix": "/api" },
      "requires": { "provider_name": "my_provider" }
    }]
  }
}
```

Per-route override (make JWT optional):
```json
"typedPerFilterConfig": {
  "envoy.filters.http.jwt_authn": {
    "requirement_name": "allow_missing"
  }
}
```

### CORS

Listener-level:
```json
{
  "name": "envoy.filters.http.cors",
  "filter": {
    "type": "cors",
    "allow_origin_string_match": [
      { "exact": "https://app.example.com" }
    ],
    "allow_methods": "GET, POST, PUT, DELETE, OPTIONS",
    "allow_headers": "Authorization, Content-Type",
    "max_age": "3600",
    "allow_credentials": true
  }
}
```

### Distributed Rate Limit

Per-route override (different domain per tier):
```json
"typedPerFilterConfig": {
  "envoy.filters.http.ratelimit": {
    "domain": "premium-tier",
    "include_vh_rate_limits": false
  }
}
```

### Ext Proc

Per-route override (disable for health checks):
```json
"typedPerFilterConfig": {
  "envoy.filters.http.ext_proc": {
    "disabled": true
  }
}
```
