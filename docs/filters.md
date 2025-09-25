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
