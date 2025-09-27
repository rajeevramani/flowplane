# HTTP Filters

This control plane exposes Envoy HTTP filters through structured JSON models. The registry keeps filters ordered, guarantees the router filter is appended last, and translates configs into the correct protobuf type URLs.

## Registry Model
* `HttpFilterConfigEntry` carries `name`, `isOptional`, `disabled`, and a tagged `filter` payload.
* If the router filter is missing, it is appended automatically. Multiple router entries trigger a validation error.
* Custom filters can still be supplied by providing a `TypedConfig` (type URL + base64 payload), but the goal is to cover common Envoy filters natively.

## Local Rate Limit
The Local Rate Limit filter mirrors `envoy.extensions.filters.http.local_ratelimit.v3.LocalRateLimit`. Field names are snake_case in JSON and map directly to Envoy’s protobuf fields.

Key fields:

| Field | Description |
| ----- | ----------- |
| `stat_prefix` | Required prefix for emitted stats. |
| `token_bucket` | Required bucket (`max_tokens`, optional `tokens_per_fill`, `fill_interval_ms`). Validation rejects zero/negative intervals. |
| `status_code` | Optional HTTP status override (clamped to 400–599). |
| `filter_enabled` / `filter_enforced` | Runtime fractional percent wrappers (numerator + denominator + runtime key). |
| `per_downstream_connection` | Switch between global and per-connection buckets. |
| `always_consume_default_token_bucket`, `max_dynamic_descriptors`, etc. | Optional toggles that map 1:1 with Envoy.

### Global vs. Route-Specific Limits

**Listener-wide limit** – add a `local_rate_limit` filter to the HTTP filter chain:

```json
{
  "name": "envoy.filters.http.local_ratelimit",
  "filter": {
    "type": "local_rate_limit",
    "stat_prefix": "listener_global",
    "token_bucket": {
      "max_tokens": 100,
      "tokens_per_fill": 100,
      "fill_interval_ms": 1000
    }
  }
}
```

All requests traversing that connection manager share the same bucket.

**Route-specific limit** – attach a scoped config through `typedPerFilterConfig`:

```json
"typedPerFilterConfig": {
  "envoy.filters.http.local_ratelimit": {
    "stat_prefix": "per_route",
    "token_bucket": {
      "max_tokens": 20,
      "tokens_per_fill": 20,
      "fill_interval_ms": 1000
    },
    "status_code": 429
  }
}
```

Route, virtual host, and weighted cluster entries can all supply scoped configs. The control plane converts these blocks into the correct `Any` payload for Envoy.

## CORS (Cross-Origin Resource Sharing)
The CORS filter maps to `envoy.extensions.filters.http.cors.v3.CorsPolicy` and is exposed through `CorsConfig`. Policies can be applied globally via the filter chain and overridden per route with `CorsPerRouteConfig`.

Key fields:

| Field | Description |
| ----- | ----------- |
| `allow_origin` | Required list of origin matchers (`exact`, `prefix`, `suffix`, `contains`, `regex`). Multiple entries are OR’d. |
| `allow_methods` | Optional list of HTTP methods (e.g. `GET`, `POST`). `*` is permitted for wildcard methods. |
| `allow_headers` / `expose_headers` | Optional header allowlists. Entries are validated against HTTP header syntax; `*` is allowed. |
| `max_age` | Optional max-age in seconds for preflight caching. Values must be non-negative and ≤ 315 576 000 000 (10,000 years). |
| `allow_credentials` | Enables credentialed requests. Validation rejects the configuration if credentials are allowed while an `*` origin matcher is present. |
| `filter_enabled` / `shadow_enabled` | Runtime fractional percent wrappers controlling enforcement vs. shadow evaluation. |
| `allow_private_network_access` | Propagates Chrome’s Private Network Access response headers when set. |
| `forward_not_matching_preflights` | Controls whether unmatched preflight requests are forwarded upstream (defaults to Envoy’s behaviour when omitted). |

### Example Filter Entry

```json
{
  "name": "envoy.filters.http.cors",
  "filter": {
    "type": "cors",
    "policy": {
      "allow_origin": [
        { "type": "exact", "value": "https://app.example.com" },
        { "type": "suffix", "value": ".internal.example.com" }
      ],
      "allow_methods": ["GET", "POST"],
      "allow_headers": ["authorization", "content-type"],
      "max_age": 600,
      "allow_credentials": true,
      "forward_not_matching_preflights": true
    }
  }
}
```

### Per-Route Override

Attach overrides using Envoy’s `typedPerFilterConfig` on routes, virtual hosts, or weighted clusters:

```json
"typedPerFilterConfig": {
  "envoy.filters.http.cors": {
    "policy": {
      "allow_origin": [
        { "type": "exact", "value": "https://reports.example.net" }
      ],
      "allow_methods": ["GET"],
      "allow_credentials": false
    }
  }
}
```

The registry validates origin patterns, headers, and max-age values before producing the relevant `Any` payload (`envoy.config.route.v3.CorsPolicy`).

## JWT Authentication
Structured JWT auth lives in `JwtAuthenticationConfig` and `JwtProviderConfig`, mapping to `envoy.extensions.filters.http.jwt_authn.v3.JwtAuthentication`.

### Providers
* Support remote JWKS (`uri`, `cluster`, `timeoutMs`, optional cache duration, async fetch, retry policy) and local JWKS (filename, inline string/bytes, env var).
* Validate non-empty headers, claim mappings, and metadata keys.
* Offer payload/header metadata emission, failure status metadata, and claim-to-header forwarding with optional padding.
* Enable payload normalization (space-delimited claims -> arrays) and provider-level JWT caching.

### Requirements
`JwtRequirementConfig` composes requirements using `provider_name`, provider + audiences, AND/OR lists, and `allow_missing` / `allow_missing_or_failed`. Route rules can inline a requirement or reference a named requirement from `requirement_map`.

### Per-Route Overrides
`JwtPerRouteConfig` supports `disabled: true` or `requirementName`. The registry handles decoding/encoding of `PerRouteConfig` protos so you can attach overrides through `typedPerFilterConfig` at route, virtual host, or weighted-cluster scopes.

### Example
See the [README quick start](../README.md#example-configuration) for listener + route JSON demonstrating JWT auth plus Local Rate Limit.

## Adding a New Filter
1. Create a module in `src/xds/filters/http/` with serializable structs and `to_any()/from_proto` helpers.
2. Register it in `src/xds/filters/http/mod.rs` by extending `HttpFilterKind` and, if needed, `HttpScopedConfig`.
3. Add unit tests covering successful conversion, validation failures, and Any round-trips.
4. Document the filter here with usage examples.

This pattern keeps configuration ergonomic while maintaining full fidelity with Envoy’s proto surface.
