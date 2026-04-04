# Filter Types Reference

## Overview

Flowplane supports 11 HTTP filter types (10 implemented, 1 defined but not yet implemented). Each maps to an Envoy HTTP filter and is implemented in `src/xds/filters/http/`. Filters are standalone resources — create once, attach to multiple listeners or route configs.

**Implemented filter types** (from `FilterType` enum in `src/domain/filter.rs`):
`header_mutation`, `jwt_auth`, `cors`, `compressor`, `local_rate_limit`, `ext_authz`, `rbac`, `oauth2`, `custom_response`, `mcp`

**Defined but not implemented**: `rate_limit` (external/distributed — API rejects with 400)

**NOT FilterTypes** (XDS modules exist but not in enum): `credential_injector`, `ext_proc`, `health_check`, `rate_limit_quota`, `wasm`, `router`

## Auth Filters

### `oauth2`
**Envoy:** `envoy.filters.http.oauth2`
**Per-route:** No (Envoy rejects per-route config for OAuth2)
**Purpose:** OAuth2 authorization code flow. Redirects unauthenticated users to an OAuth2 provider. Listener-only attachment.
**Source:** `src/xds/filters/http/oauth2.rs`

```json
{
  "filterType": "oauth2",
  "config": {
    "type": "oauth2",
    "config": {
      "token_endpoint": "https://auth.example.com/oauth/token",
      "authorization_endpoint": "https://auth.example.com/authorize",
      "redirect_uri": "https://app.example.com/callback",
      "credentials": { "client_id": "my-app", "client_secret_ref": "oauth-secret" },
      "forward_bearer_token": true,
      "auth_scopes": ["openid", "profile"]
    }
  }
}
```

**Gotcha:** OAuth2 hardcodes `hmac-secret` as the SDS secret name for cookie signing in `to_any()` — not user-configurable.

### `jwt_auth`
**Envoy:** `envoy.filters.http.jwt_authn`
**Per-route:** Yes (reference_only — rules reference, not full config override)
**Purpose:** JWT validation with JWKS provider. Validates tokens and extracts claims.
**Source:** `src/xds/filters/http/jwt_auth.rs`

```json
{
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
              "cluster": "jwks-cluster",
              "timeout_ms": 5000
            }
          },
          "cache_duration_seconds": 300,
          "fromHeaders": [{ "name": "Authorization", "value_prefix": "Bearer " }],
          "forward": true
        }
      },
      "rules": [
        { "match": { "path": { "Prefix": "/api" } }, "requires": { "type": "provider_name", "provider_name": "my-provider" } }
      ]
    }
  }
}
```

**Gotcha:** A JWT filter without `rules` does NOT enforce authentication — all traffic passes through.
**Gotcha:** Local JWKS requires at least one valid public key — `{"keys":[]}` and malformed keys cause Envoy NACK.
**Gotcha:** Path match in rules uses PascalCase: `{"Prefix": "/api"}`, not `{"prefix": "/api"}`.

### `ext_authz`
**Envoy:** `envoy.filters.http.ext_authz`
**Per-route:** Yes (full_config_override)
**Purpose:** External authorization service. Sends auth decisions to an external gRPC or HTTP service.
**Source:** `src/xds/filters/http/ext_authz.rs`

```json
{
  "filterType": "ext_authz",
  "config": {
    "type": "ext_authz",
    "config": {
      "service": {
        "type": "http",
        "server_uri": {
          "uri": "https://authz.example.com/check",
          "cluster": "authz-service",
          "timeout_ms": 250
        }
      },
      "authorization_request": {
        "allowed_headers": ["Authorization", "X-Request-Id"]
      },
      "failure_mode_allow": false
    }
  }
}
```

**Gotcha:** gRPC mode `target_uri` is an Envoy cluster name (not a URL) — the cluster must exist or requests will fail.
**Gotcha:** With `failure_mode_allow: false`, returns 403 when the authz service is unreachable.

### `rbac`
**Envoy:** `envoy.filters.http.rbac`
**Per-route:** Yes (full_config_override)
**Purpose:** Role-based access control. Allows/denies requests based on principals, permissions, and conditions.
**Source:** `src/xds/filters/http/rbac.rs`

```json
{
  "filterType": "rbac",
  "config": {
    "type": "rbac",
    "config": {
      "rules": {
        "action": "ALLOW",
        "policies": {
          "allow-internal": {
            "permissions": [{ "any": true }],
            "principals": [{ "source_ip": { "address_prefix": "10.0.0.0", "prefix_len": 8 } }]
          }
        }
      }
    }
  }
}
```

**Gotcha:** RBAC requires at least `rules` or `shadow_rules` — empty config fails validation.
**Gotcha:** Permission/principal types use tagged union with `"type"` field: `{"type": "header", "name": ":method", "exact_match": "GET"}`.

### ~~`credential_injector`~~ (NOT a FilterType)
**Status:** XDS module exists (`src/xds/filters/http/credential_injector.rs`) but is NOT registered in the `FilterType` enum in `src/domain/filter.rs`. Cannot be created via API — returns 400 with "Invalid filter type". Use `header_mutation` as a workaround to inject credentials into upstream requests.

## Rate Limiting Filters

### `local_rate_limit`
**Envoy:** `envoy.filters.http.local_ratelimit`
**Per-route:** Yes (full_config_override)
**Purpose:** In-process token bucket rate limiting. No external service needed.
**Source:** `src/xds/filters/http/local_rate_limit.rs`

```json
{
  "filterType": "local_rate_limit",
  "config": {
    "type": "local_rate_limit",
    "config": {
      "stat_prefix": "api_rl",
      "token_bucket": {
        "max_tokens": 100,
        "tokens_per_fill": 100,
        "fill_interval_ms": 1000
      },
      "filter_enabled": {
        "numerator": 100,
        "denominator": "hundred"
      },
      "filter_enforced": {
        "numerator": 100,
        "denominator": "hundred"
      }
    }
  }
}
```

**Gotcha:** `filter_enabled` and `filter_enforced` both default to disabled — you must explicitly set both to 100% for the filter to work.
**Gotcha:** Denominator values are snake_case: `hundred`, `ten_thousand`, `million` — NOT SCREAMING_CASE.

### `rate_limit`
**Envoy:** `envoy.filters.http.ratelimit`
**Per-route:** Yes
**Status:** Defined in FilterType enum but **not implemented** — API rejects creation with 400 error. Use `local_rate_limit` for rate limiting.
**Source:** `src/xds/filters/http/rate_limit.rs`

## Manipulation Filters

### `header_mutation`
**Envoy:** `envoy.filters.http.header_mutation`
**Per-route:** Yes (full_config_override)
**Purpose:** Add, remove, or modify request/response headers. Does NOT require listener-level config (`requires_listener_config: false`).
**Source:** `src/xds/filters/http/header_mutation.rs`

```json
{
  "filterType": "header_mutation",
  "config": {
    "type": "header_mutation",
    "config": {
      "request_headers_to_add": [
        { "key": "X-Custom", "value": "added-by-gateway", "append": true }
      ],
      "request_headers_to_remove": ["X-Internal"],
      "response_headers_to_add": [
        { "key": "X-Served-By", "value": "flowplane", "append": false }
      ],
      "response_headers_to_remove": ["Server"]
    }
  }
}
```

**Gotcha:** `response_headers_to_remove: ["server"]` doesn't remove `server: envoy` — Envoy adds it after filter processing.

### `cors`
**Envoy:** `envoy.filters.http.cors`
**Per-route:** Yes (disable only)
**Purpose:** Cross-origin resource sharing policy.
**Source:** `src/xds/filters/http/cors.rs`

```json
{
  "filterType": "cors",
  "config": {
    "type": "cors",
    "config": {
      "policy": {
        "allow_origin": [
          { "type": "exact", "value": "https://app.example.com" }
        ],
        "allow_methods": ["GET", "POST", "PUT", "DELETE"],
        "allow_headers": ["Authorization", "Content-Type"],
        "max_age": 86400
      }
    }
  }
}
```

**Gotcha:** CORS config has an extra nesting level: `config.config.policy` wraps the actual policy fields.
**Gotcha:** Origin matchers use a tagged enum: `{"type": "exact", "value": "..."}` — not bare strings.

### `custom_response`
**Envoy:** `envoy.filters.http.custom_response`
**Per-route:** Yes (disable only)
**Purpose:** Custom error pages and responses based on status codes.
**Source:** `src/xds/filters/http/custom_response.rs`

```json
{
  "filterType": "custom_response",
  "config": {
    "type": "custom_response",
    "config": {
      "matchers": [
        {
          "status_code": { "type": "exact", "code": 503 },
          "response": {
            "status_code": 503,
            "body": "{\"error\": \"Service unavailable\"}",
            "headers": { "content-type": "application/json" }
          }
        }
      ]
    }
  }
}
```

**Gotcha:** Status code matcher uses tagged union: `{"type": "exact", "code": 503}` or `{"type": "range", "min": 500, "max": 599}`.
**Gotcha:** `response.body` must not be empty string — validation rejects it.

## Processing Filters

### `compressor`
**Envoy:** `envoy.filters.http.compressor`
**Per-route:** Yes (disable only)
**Purpose:** Response compression (gzip).
**Source:** `src/xds/filters/http/compressor.rs`

```json
{
  "filterType": "compressor",
  "config": {
    "type": "compressor",
    "config": {
      "response_direction_config": {
        "common_config": {
          "min_content_length": 1024
        }
      },
      "compressor_library": {
        "type": "gzip",
        "compression_level": "default_compression",
        "window_bits": 15,
        "memory_level": 8
      }
    }
  }
}
```

**Gotcha:** Compressor library uses tagged enum with flat fields: `{"type": "gzip", "compression_level": "..."}` — not nested like other filter configs.
**Known bug:** Envoy NACKs on attach — `RuntimeFeatureFlag.runtime_key` is empty string in `compressor.rs:to_any()`. Envoy proto validation requires >= 1 character.

## Protocol Filters

### `mcp`
**Envoy:** `envoy.filters.http.mcp`
**Per-route:** Yes (disable only)
**Purpose:** MCP (Model Context Protocol) integration filter for AI/LLM gateway traffic. Inspects and validates JSON-RPC 2.0 and SSE stream traffic. Two modes: `pass_through` (inspect only) and `reject_no_mcp` (block non-MCP traffic, requires custom Envoy extension).
**Source:** `src/xds/filters/http/mcp.rs`

**Gotcha:** The old docs incorrectly stated the Envoy name was `envoy.filters.http.ext_proc` — it is `envoy.filters.http.mcp`.
**Gotcha:** `reject_no_mcp` mode requires custom Envoy extension compilation — standard dev image may not include it.

## Adding a New Filter Type

Flowplane has a **dual representation** for filters:
- **Compiled** — Rust `FilterType` enum + `HttpFilterKind` with typed config structs and protobuf conversion
- **Runtime schema** — YAML files in `filter-schemas/built-in/` loaded by `FilterSchemaDefinition` (`src/domain/filter_schema.rs`) for validation and `cp_list_filter_types`/`cp_get_filter_type` responses

Both must be updated when adding a new filter type.

### Step-by-step checklist

1. **Domain enum** — Add variant to `FilterType` in `src/domain/filter.rs`:
   - Add to the enum itself
   - Add `FilterTypeMetadata` entry in `filter_registry()` (Envoy filter name, type URLs, attachment points, per-route behavior)
   - Update `fmt::Display`, `FromStr`, `from_http_filter_name()` static array

2. **xDS implementation** — Create `src/xds/filters/http/<name>.rs`:
   - Define typed config struct (e.g., `IpAllowlistConfig`)
   - Implement `to_any()` — convert config JSON to Envoy protobuf `Any`
   - If per-route supported: define per-route config struct with its own `to_any()`/`from_any()`
   - Follow an existing filter as a template (e.g., `rbac.rs` for auth filters, `local_rate_limit.rs` for simple configs)

3. **Register in mod.rs** — Update `src/xds/filters/http/mod.rs`:
   - Add `pub mod <name>;`
   - Add variant to `HttpFilterKind` enum (listener-level config)
   - Add variant to `HttpScopedConfig` enum (per-route config, if supported)
   - Update `HttpFilterKind::default_name()`, `to_any()`, `from_any()` methods
   - Update `HttpScopedConfig::to_any()`, `from_any()` methods

4. **Filter schema YAML** — Create `filter-schemas/built-in/<name>.yaml`:
   - Defines the JSON schema for `cp_list_filter_types`/`cp_get_filter_type`
   - Validated by `FilterConfigValidator` in `src/services/filter_validation.rs`
   - Includes: name, Envoy mapping, capabilities, config_schema (JSON Schema format)

5. **Tests**:
   - Unit tests for protobuf conversion (`src/xds/filters/http/<name>.rs`)
   - Unit tests for `FilterType` roundtrip (Display + FromStr)
   - Integration tests for create → attach → verify via API
   - E2E smoke test against running stack

6. **Update skills** — Update this reference and `flowplane-api` filter table

## Secrets and SDS

Filters that need credentials (`oauth2`, `jwt_auth`, `ext_authz`) reference secrets by name. Secrets are encrypted in PostgreSQL and delivered to Envoy via SDS over ADS.

See the **`flowplane-secrets` skill** for the full reference: secret types, CLI commands, MCP tools, REST API, rotation, encryption key management, filter integration examples, and source files.

Quick reference — filters that use secrets:

| Filter | Secret Field | Secret Type |
|---|---|---|
| `oauth2` | `token_secret.name` | `generic_secret` |
| `jwt_auth` | `jwks` (Sds variant) | `generic_secret` |
| `ext_authz` | `auth_secret.name` / `tls_secret.name` | `generic_secret` / `tls_certificate` |

Note: `credential_injector` has an XDS module with SDS support (`src/xds/filters/http/credential_injector.rs`) but is NOT a registered FilterType — cannot be created via API.

## Filter Execution Order

Filters execute in the order they appear in the listener's filter chain (`order` field in `filter_attachments`). Recommended order:

1. CORS (handle preflight before auth)
2. Auth filters (oauth2, jwt_auth, ext_authz, rbac)
3. Rate limiting (after auth — don't rate-limit unauthenticated requests)
4. Header mutation
5. Compression
6. Router (auto-appended by Flowplane — never add manually)
