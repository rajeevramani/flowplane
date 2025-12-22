# JWT Authentication Filter

The JWT Authentication filter validates JSON Web Tokens (JWT) on incoming requests. It supports multiple JWT providers, remote and local JWKS sources, and flexible requirement rules to control which routes require authentication.

## Envoy Documentation

- [JWT Authentication Filter Reference](https://www.envoyproxy.io/docs/envoy/latest/configuration/http/http_filters/jwt_authn_filter)
- [JWT Authentication Filter API](https://www.envoyproxy.io/docs/envoy/latest/api-v3/extensions/filters/http/jwt_authn/v3/config.proto)

## How It Works in Envoy

The JWT Authentication filter validates tokens by:

```
┌─────────┐     ┌─────────┐     ┌──────────────┐     ┌─────────────┐
│  User   │     │  Envoy  │     │ JWKS Endpoint│     │  Upstream   │
└────┬────┘     └────┬────┘     └──────┬───────┘     └──────┬──────┘
     │               │                 │                    │
     │ 1. Request    │                 │                    │
     │   + JWT       │                 │                    │
     ├──────────────►│                 │                    │
     │               │                 │                    │
     │               │ 2. Fetch JWKS   │                    │
     │               │   (if cached)   │                    │
     │               ├────────────────►│                    │
     │               │                 │                    │
     │               │ 3. JWKS         │                    │
     │               │◄────────────────┤                    │
     │               │                 │                    │
     │               │ 4. Validate JWT │                    │
     │               │   - Signature   │                    │
     │               │   - Issuer      │                    │
     │               │   - Audience    │                    │
     │               │   - Expiration  │                    │
     │               │                 │                    │
     │               │ 5. Forward      │                    │
     │               ├─────────────────────────────────────►│
     │               │                 │                    │
     │ 6. Response   │◄────────────────────────────────────┤
     │◄──────────────┤                 │                    │
```

### Key Behaviors

1. **Token Extraction**: JWTs can be extracted from Authorization headers, custom headers, query parameters, or cookies
2. **JWKS Caching**: Remote JWKS are cached with configurable duration and async refresh
3. **Claim Forwarding**: JWT claims can be forwarded to upstream services as headers
4. **Metadata Injection**: JWT payload and header can be stored in Envoy's dynamic metadata
5. **Flexible Requirements**: Routes can require specific providers, any of several providers, or allow missing tokens

### Per-Route Support

**The JWT Authentication filter supports per-route configuration** via `typed_per_filter_config`. You can:
- Disable JWT validation for specific routes
- Reference different named requirements for different routes

## Flowplane Configuration

### Top-Level Configuration Fields

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `providers` | object | Yes | - | Map of named JWT provider configurations |
| `rules` | array | No | `[]` | Ordered list of rules mapping routes to requirements |
| `requirement_map` | object | No | `{}` | Reusable named requirement definitions |
| `filter_state_rules` | object | No | - | Dynamic requirement selection via filter state |
| `bypass_cors_preflight` | boolean | No | `false` | Skip JWT validation for CORS preflight requests |
| `strip_failure_response` | boolean | No | `false` | Remove WWW-Authenticate details from 401 responses |
| `stat_prefix` | string | No | - | Custom statistics prefix |

### Provider Configuration

Each provider in the `providers` map supports:

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `jwks` | object | Yes | - | JWKS source configuration (remote or local) |
| `issuer` | string | No | - | Expected token issuer (`iss` claim) |
| `audiences` | array | No | `[]` | Allowed audience values (`aud` claim) |
| `subjects` | object | No | - | Subject matcher configuration |
| `require_expiration` | boolean | No | `false` | Require `exp` claim to be present |
| `max_lifetime_seconds` | integer | No | - | Maximum allowed token lifetime |
| `clock_skew_seconds` | integer | No | `60` | Tolerance for clock drift in exp/nbf validation |
| `forward` | boolean | No | `false` | Forward the original JWT to upstream |
| `from_headers` | array | No | `[]` | Custom headers to extract JWT from |
| `from_params` | array | No | `[]` | Query parameters to extract JWT from |
| `from_cookies` | array | No | `[]` | Cookies to extract JWT from |
| `forward_payload_header` | string | No | - | Header name for forwarded base64url payload |
| `pad_forward_payload_header` | boolean | No | `false` | Add padding to forwarded payload |
| `payload_in_metadata` | string | No | - | Metadata key for JWT payload |
| `header_in_metadata` | string | No | - | Metadata key for JWT header |
| `failed_status_in_metadata` | string | No | - | Metadata key for failure info |
| `normalize_payload_in_metadata` | object | No | - | Payload normalization options |
| `jwt_cache_config` | object | No | - | Token caching configuration |
| `claim_to_headers` | array | No | `[]` | Claims to forward as headers |
| `clear_route_cache` | boolean | No | `false` | Clear routing cache on metadata update |

### JWKS Source Configuration

JWKS can be fetched remotely or provided locally.

#### Remote JWKS

```json
{
  "jwks": {
    "type": "remote",
    "http_uri": {
      "uri": "https://auth.example.com/.well-known/jwks.json",
      "cluster": "auth-cluster",
      "timeout_ms": 5000
    },
    "cache_duration_seconds": 600,
    "async_fetch": {
      "fast_listener": true,
      "failed_refetch_duration_seconds": 5
    },
    "retry_policy": {
      "num_retries": 3,
      "retry_backoff": {
        "base_interval_ms": 200,
        "max_interval_ms": 2000
      }
    }
  }
}
```

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `http_uri.uri` | string | Yes | - | URL to fetch JWKS from |
| `http_uri.cluster` | string | Yes | - | Cluster name for JWKS endpoint |
| `http_uri.timeout_ms` | integer | No | `1000` | Request timeout in milliseconds |
| `cache_duration_seconds` | integer | No | - | How long to cache JWKS |
| `async_fetch.fast_listener` | boolean | No | `false` | Start listener before JWKS is fetched |
| `async_fetch.failed_refetch_duration_seconds` | integer | No | - | Retry interval on fetch failure |
| `retry_policy.num_retries` | integer | No | - | Number of retry attempts |
| `retry_policy.retry_backoff.base_interval_ms` | integer | No | - | Base retry interval |
| `retry_policy.retry_backoff.max_interval_ms` | integer | No | - | Maximum retry interval |

#### Local JWKS

Local JWKS can be provided inline, from a file, or from an environment variable. Exactly one source must be specified.

```json
{
  "jwks": {
    "type": "local",
    "inline_string": "{\"keys\":[{\"kty\":\"RSA\",\"n\":\"...\",\"e\":\"AQAB\",\"kid\":\"key1\"}]}"
  }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `filename` | string | Path to file containing JWKS JSON |
| `inline_string` | string | JWKS JSON content inline |
| `inline_bytes` | string | Base64-encoded JWKS content |
| `environment_variable` | string | Environment variable containing JWKS |

### Header Extraction Configuration

By default, JWTs are extracted from the `Authorization` header with `Bearer ` prefix. Custom headers can be configured:

```json
{
  "from_headers": [
    {
      "name": "Authorization",
      "value_prefix": "Bearer "
    },
    {
      "name": "X-Custom-Token"
    }
  ]
}
```

### Claim to Header Configuration

Forward JWT claims as headers to upstream:

```json
{
  "claim_to_headers": [
    {
      "header_name": "x-jwt-sub",
      "claim_name": "sub"
    },
    {
      "header_name": "x-jwt-email",
      "claim_name": "email"
    }
  ]
}
```

### Subject Matcher Configuration

Match the subject claim against patterns:

```json
{
  "subjects": {
    "type": "prefix",
    "value": "spiffe://example.com/"
  }
}
```

Supported match types: `exact`, `prefix`, `suffix`, `contains`, `regex`

### JWT Cache Configuration

Configure per-provider token caching:

```json
{
  "jwt_cache_config": {
    "jwt_cache_size": 500,
    "jwt_max_token_size": 8192
  }
}
```

### Requirement Rules

Rules map route matches to JWT requirements:

```json
{
  "rules": [
    {
      "match": {
        "path": {"type": "prefix", "value": "/api/public"}
      },
      "requires": {
        "type": "allow_missing"
      }
    },
    {
      "match": {
        "path": {"type": "prefix", "value": "/api"}
      },
      "requires": {
        "type": "provider_name",
        "provider_name": "auth0"
      }
    }
  ]
}
```

### Requirement Types

| Type | Fields | Description |
|------|--------|-------------|
| `provider_name` | `provider_name` | Require JWT from a specific provider |
| `provider_with_audiences` | `provider_name`, `audiences` | Require provider with audience override |
| `requires_any` | `requirements` | Any of the nested requirements must pass |
| `requires_all` | `requirements` | All nested requirements must pass |
| `allow_missing` | - | Allow requests without JWT (reject invalid) |
| `allow_missing_or_failed` | - | Allow requests even if JWT is missing or invalid |

### Requirement Map

Define reusable requirements that can be referenced by name:

```json
{
  "requirement_map": {
    "strict": {
      "type": "provider_name",
      "provider_name": "primary"
    },
    "relaxed": {
      "type": "allow_missing"
    }
  }
}
```

Rules can reference these by name:

```json
{
  "rules": [
    {
      "match": {"path": {"type": "prefix", "value": "/api"}},
      "requirement_name": "strict"
    }
  ]
}
```

## Per-Route Configuration

JWT authentication can be customized per-route using `typed_per_filter_config`:

### Disable JWT for a Route

```json
{
  "typedPerFilterConfig": {
    "envoy.filters.http.jwt_authn": {
      "filter_type": "jwt_authn",
      "disabled": true
    }
  }
}
```

### Reference a Named Requirement

```json
{
  "typedPerFilterConfig": {
    "envoy.filters.http.jwt_authn": {
      "filter_type": "jwt_authn",
      "requirement_name": "relaxed"
    }
  }
}
```

## Prerequisites

### 1. JWKS Cluster (for Remote JWKS)

Create a cluster to reach your identity provider's JWKS endpoint:

```bash
curl -X POST http://localhost:8080/api/v1/clusters \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "team": "my-team",
    "name": "auth0-jwks",
    "serviceName": "auth0-jwks-service",
    "endpoints": [
      {"host": "your-tenant.auth0.com", "port": 443}
    ],
    "useTls": true,
    "lbPolicy": "ROUND_ROBIN"
  }'
```

## Complete Examples

### Basic JWT Validation with Auth0

```bash
curl -X POST http://localhost:8080/api/v1/listeners \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "jwt-listener",
    "address": "0.0.0.0",
    "port": 8080,
    "team": "my-team",
    "protocol": "HTTP",
    "filterChains": [{
      "name": "default",
      "filters": [{
        "name": "envoy.filters.network.http_connection_manager",
        "type": "httpConnectionManager",
        "routeConfigName": "my-routes",
        "httpFilters": [
          {
            "name": "envoy.filters.http.jwt_authn",
            "filter": {
              "type": "jwt_authn",
              "providers": {
                "auth0": {
                  "issuer": "https://your-tenant.auth0.com/",
                  "audiences": ["your-api-identifier"],
                  "jwks": {
                    "type": "remote",
                    "http_uri": {
                      "uri": "https://your-tenant.auth0.com/.well-known/jwks.json",
                      "cluster": "auth0-jwks",
                      "timeout_ms": 5000
                    },
                    "cache_duration_seconds": 600
                  },
                  "forward": true,
                  "claim_to_headers": [
                    {"header_name": "x-user-id", "claim_name": "sub"}
                  ]
                }
              },
              "rules": [
                {
                  "match": {"path": {"type": "prefix", "value": "/"}},
                  "requires": {
                    "type": "provider_name",
                    "provider_name": "auth0"
                  }
                }
              ],
              "bypass_cors_preflight": true
            }
          },
          {
            "name": "envoy.filters.http.router",
            "filter": {"type": "router"}
          }
        ]
      }]
    }]
  }'
```

### Multiple Providers with Fallback

Support tokens from multiple identity providers:

```bash
curl -X POST http://localhost:8080/api/v1/listeners \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "multi-provider-listener",
    "address": "0.0.0.0",
    "port": 8080,
    "team": "my-team",
    "protocol": "HTTP",
    "filterChains": [{
      "name": "default",
      "filters": [{
        "name": "envoy.filters.network.http_connection_manager",
        "type": "httpConnectionManager",
        "routeConfigName": "my-routes",
        "httpFilters": [
          {
            "name": "envoy.filters.http.jwt_authn",
            "filter": {
              "type": "jwt_authn",
              "providers": {
                "auth0": {
                  "issuer": "https://your-tenant.auth0.com/",
                  "audiences": ["api://my-app"],
                  "jwks": {
                    "type": "remote",
                    "http_uri": {
                      "uri": "https://your-tenant.auth0.com/.well-known/jwks.json",
                      "cluster": "auth0-jwks",
                      "timeout_ms": 5000
                    }
                  }
                },
                "keycloak": {
                  "issuer": "https://keycloak.example.com/realms/myrealm",
                  "audiences": ["my-app"],
                  "jwks": {
                    "type": "remote",
                    "http_uri": {
                      "uri": "https://keycloak.example.com/realms/myrealm/protocol/openid-connect/certs",
                      "cluster": "keycloak-jwks",
                      "timeout_ms": 5000
                    }
                  }
                }
              },
              "rules": [
                {
                  "match": {"path": {"type": "prefix", "value": "/api"}},
                  "requires": {
                    "type": "requires_any",
                    "requirements": [
                      {"type": "provider_name", "provider_name": "auth0"},
                      {"type": "provider_name", "provider_name": "keycloak"}
                    ]
                  }
                }
              ]
            }
          },
          {
            "name": "envoy.filters.http.router",
            "filter": {"type": "router"}
          }
        ]
      }]
    }]
  }'
```

### Public and Protected Routes

Different JWT requirements for different routes:

```json
{
  "type": "jwt_authn",
  "providers": {
    "primary": {
      "issuer": "https://auth.example.com/",
      "audiences": ["my-api"],
      "jwks": {
        "type": "remote",
        "http_uri": {
          "uri": "https://auth.example.com/.well-known/jwks.json",
          "cluster": "auth-jwks",
          "timeout_ms": 5000
        }
      }
    }
  },
  "requirement_map": {
    "require_jwt": {
      "type": "provider_name",
      "provider_name": "primary"
    },
    "optional_jwt": {
      "type": "allow_missing"
    }
  },
  "rules": [
    {
      "match": {"path": {"type": "exact", "value": "/healthz"}},
      "requires": {"type": "allow_missing_or_failed"}
    },
    {
      "match": {"path": {"type": "prefix", "value": "/api/public"}},
      "requirement_name": "optional_jwt"
    },
    {
      "match": {"path": {"type": "prefix", "value": "/api"}},
      "requirement_name": "require_jwt"
    }
  ],
  "bypass_cors_preflight": true
}
```

### JWT with Metadata for RBAC

Store JWT claims in metadata for use by downstream filters (like RBAC):

```json
{
  "type": "jwt_authn",
  "providers": {
    "primary": {
      "issuer": "https://auth.example.com/",
      "jwks": {
        "type": "remote",
        "http_uri": {
          "uri": "https://auth.example.com/.well-known/jwks.json",
          "cluster": "auth-jwks",
          "timeout_ms": 5000
        }
      },
      "payload_in_metadata": "jwt_payload",
      "header_in_metadata": "jwt_header",
      "normalize_payload_in_metadata": {
        "space_delimited_claims": ["scope"]
      }
    }
  },
  "rules": [
    {
      "match": {"path": {"type": "prefix", "value": "/"}},
      "requires": {
        "type": "provider_name",
        "provider_name": "primary"
      }
    }
  ]
}
```

The RBAC filter can then reference `metadata.filter_metadata["envoy.filters.http.jwt_authn"]["jwt_payload"]`.

### Local JWKS for Testing

Use inline JWKS for development or testing:

```json
{
  "type": "jwt_authn",
  "providers": {
    "local": {
      "issuer": "test-issuer",
      "jwks": {
        "type": "local",
        "inline_string": "{\"keys\":[{\"kty\":\"RSA\",\"n\":\"sXch...\",\"e\":\"AQAB\",\"kid\":\"test-key\"}]}"
      }
    }
  },
  "rules": [
    {
      "requires": {
        "type": "provider_name",
        "provider_name": "local"
      }
    }
  ]
}
```

## Per-Route Override Example

Configure routes with different JWT requirements:

```bash
curl -X POST http://localhost:8080/api/v1/routes \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "my-routes",
    "team": "my-team",
    "virtual_hosts": [{
      "name": "api",
      "domains": ["api.example.com"],
      "routes": [
        {
          "name": "health",
          "match": {"path": {"type": "exact", "value": "/healthz"}},
          "action": {"type": "forward", "cluster": "backend"},
          "typedPerFilterConfig": {
            "envoy.filters.http.jwt_authn": {
              "filter_type": "jwt_authn",
              "disabled": true
            }
          }
        },
        {
          "name": "admin",
          "match": {"path": {"type": "prefix", "value": "/admin"}},
          "action": {"type": "forward", "cluster": "backend"},
          "typedPerFilterConfig": {
            "envoy.filters.http.jwt_authn": {
              "filter_type": "jwt_authn",
              "requirement_name": "admin_jwt"
            }
          }
        },
        {
          "name": "api",
          "match": {"path": {"type": "prefix", "value": "/"}},
          "action": {"type": "forward", "cluster": "backend"}
        }
      ]
    }]
  }'
```

## Troubleshooting

### Common Issues

1. **401 Unauthorized - Token Not Found**

   Check token extraction configuration:
   - Default extraction is from `Authorization: Bearer <token>`
   - Verify `from_headers`, `from_params`, or `from_cookies` if using custom locations

2. **401 Unauthorized - Invalid Signature**

   JWKS issue:
   ```bash
   # Check if JWKS cluster is healthy
   curl -s "http://localhost:9902/clusters" | grep jwks

   # Check JWKS fetch stats
   curl -s "http://localhost:9902/stats" | grep jwt_authn
   ```

3. **401 Unauthorized - Token Expired**

   - Verify server time sync
   - Increase `clock_skew_seconds` if clock drift is an issue
   - Check token `exp` claim value

4. **401 Unauthorized - Audience Mismatch**

   Verify the `audiences` array includes the token's `aud` claim value.

5. **JWKS Fetch Timeout**

   ```bash
   # Check cluster connectivity
   curl -s "http://localhost:9902/clusters" | grep -A5 auth-jwks
   ```

   - Verify cluster TLS configuration
   - Increase `timeout_ms` if needed
   - Check network connectivity to JWKS endpoint

6. **Listener Fails to Start (Missing JWKS)**

   With `async_fetch.fast_listener: false` (default), the listener waits for JWKS. Set to `true` to start immediately.

### Debug Checklist

```bash
# 1. Check JWT filter is in the config
curl -s "http://localhost:9902/config_dump?resource=dynamic_listeners" | \
  jq '.configs[].active_state.listener.filter_chains[].filters[].typed_config.http_filters[] | select(.name == "envoy.filters.http.jwt_authn")'

# 2. Check JWKS cluster health
curl -s "http://localhost:9902/clusters" | grep -A10 jwks

# 3. Check JWT stats
curl -s "http://localhost:9902/stats" | grep jwt

# 4. Test token manually
jwt decode <your-token>  # Using jwt-cli or similar tool

# 5. Verify issuer/audience
jwt decode <your-token> | jq '.payload | {iss, aud}'
```

### Metrics

With JWT authentication enabled, Envoy emits metrics:

| Metric | Description |
|--------|-------------|
| `jwt_authn.{provider}.allowed` | Requests with valid JWT |
| `jwt_authn.{provider}.denied` | Requests denied due to JWT issues |
| `jwt_authn.{provider}.jwks_fetch_success` | Successful JWKS fetches |
| `jwt_authn.{provider}.jwks_fetch_failed` | Failed JWKS fetches |

## Security Considerations

1. **JWKS Endpoint Security**: Always use HTTPS for remote JWKS endpoints
2. **Audience Validation**: Always specify `audiences` to prevent token reuse across APIs
3. **Issuer Validation**: Always specify `issuer` to ensure tokens are from trusted sources
4. **Token Expiration**: Enable `require_expiration` for production deployments
5. **Clock Skew**: Keep `clock_skew_seconds` minimal (default 60s is usually sufficient)
6. **JWKS Caching**: Balance between security (shorter cache) and performance (longer cache)

## See Also

- [Filters Overview](../filters.md) - All available filters
- [RBAC Filter](./rbac.md) - Role-based access control using JWT claims
- [External Authorization](./ext_authz.md) - External auth service integration
