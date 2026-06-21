> Audience: platform-engineers · Status: stable

# HTTP Filter Catalogue

Reference for the closed set of HTTP filters flowplane-v2 accepts in a listener filter chain, the per-scope overrides each supports, the Envoy filter URI each maps to, and the filters Flowplane injects automatically.

## Filter chain model

- A listener's chain is a list of `HttpFilterEntry`. Each entry is one `HttpFilterSpec` (tagged in JSON by `type`, `snake_case`) plus a `disabled` boolean.
- `HttpFilterEntry`:
  - `filter` (`HttpFilterSpec`, required) — the filter and its config.
  - `disabled` (bool, optional, default `false`) — when `true` the filter stays in the chain but Envoy skips it (toggle without re-ordering). Serialized only when `true`.
- Chain order is semantic: filters run in declared order.
- Chain invariant (`validate_filter_chain`): each filter `type` may appear **at most once per listener**; duplicates are rejected (`duplicate filter type "…" in the chain`).
- All structs use `deny_unknown_fields` — unknown JSON keys are rejected.

The filter vocabulary is closed. There are 9 declared filter kinds (`HttpFilterKind`): `cors`, `local_rate_limit`, `header_mutation`, `health_check`, `compressor`, `jwt_auth`, `ext_authz`, `rbac`, `global_rate_limit`.

## Declared filters

### cors (`HttpFilterSpec::Cors` → `CorsConfig`)

Chain marker only — the chain entry translates to an empty Envoy `Cors` message. The active policy is read from per-scope `filter_overrides` (Envoy reads CORS policy exclusively from per-route config). The chain-level `CorsConfig` is validated but documents the default policy only.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `allow_origin` | `Vec<OriginMatcher>` | required, non-empty | Origin matchers. |
| `allow_methods` | `Vec<String>` | optional (default empty) | Allowed methods. |
| `allow_headers` | `Vec<String>` | optional (default empty) | Allowed request headers. |
| `expose_headers` | `Vec<String>` | optional (default empty) | Headers exposed to the client. |
| `max_age_seconds` | `Option<u64>` | optional | Preflight cache duration. |
| `allow_credentials` | `bool` | optional (default `false`) | Allow credentialed requests. |

`OriginMatcher` (tagged by `match`, `snake_case`): `exact { value }`, `prefix { value }`, `suffix { value }`, `contains { value }`.

Validation:
- `allow_origin` must list at least one matcher and at most 64 (`MAX_CORS_ORIGINS`).
- Each matcher value: 1..=2048 characters (`MAX_CORS_ORIGIN_VALUE_LEN`), no control characters.
- `allow_methods`, `allow_headers`, `expose_headers`: at most 128 values each (`MAX_CORS_LIST_VALUES`); each value 1..=256 characters (`MAX_CORS_TOKEN_VALUE_LEN`), no control characters.
- `allow_credentials` cannot be combined with a wildcard origin (an `exact` or `prefix` matcher whose value is `*`).
- `max_age_seconds` must not exceed `315576000000` (`MAX_AGE_CAP`).

### local_rate_limit (`HttpFilterSpec::LocalRateLimit` → `LocalRateLimitConfig`)

| Field | Type | Required | Meaning |
|---|---|---|---|
| `stat_prefix` | `String` | required | Stats prefix. |
| `token_bucket` | `TokenBucket` | required | Token bucket settings. |
| `status_code` | `Option<u16>` | optional | Rejection status; Envoy default 429 when omitted. |

`TokenBucket`:

| Field | Type | Required | Meaning |
|---|---|---|---|
| `max_tokens` | `u32` | required | Bucket capacity. |
| `tokens_per_fill` | `Option<u32>` | optional | Tokens added per fill; defaults to `max_tokens` when omitted. |
| `fill_interval_ms` | `u64` | required | Fill interval in milliseconds. |

Validation:
- `stat_prefix` must be non-empty.
- `token_bucket.max_tokens` must be >= 1.
- `token_bucket.fill_interval_ms` must be > 0.
- `status_code`, if present, must be in 400..=599.

### header_mutation (`HttpFilterSpec::HeaderMutation` → `HeaderMutationConfig`)

| Field | Type | Required | Meaning |
|---|---|---|---|
| `request_headers_to_add` | `Vec<HeaderValue>` | optional (default empty) | Request headers to add/append. |
| `request_headers_to_remove` | `Vec<String>` | optional (default empty) | Request header names to remove. |
| `response_headers_to_add` | `Vec<HeaderValue>` | optional (default empty) | Response headers to add/append. |
| `response_headers_to_remove` | `Vec<String>` | optional (default empty) | Response header names to remove. |

`HeaderValue`:

| Field | Type | Required | Meaning |
|---|---|---|---|
| `key` | `String` | required | Header name. |
| `value` | `String` | required | Header value. |
| `append` | `bool` | optional (default `false`) | `true` appends if exists or adds; `false` overwrites if exists or adds. |

Validation:
- Each `*_add` and `*_remove` list: at most 128 entries (`MAX_HEADER_MUTATIONS_PER_DIRECTION`).
- Each header key (for adds) and each remove entry: 1..=256 characters (`MAX_HEADER_NAME_LEN`), no control characters.
- Each header value (for adds): 1..=4096 characters (`MAX_HEADER_VALUE_LEN`), no control characters.
- Every add entry's `key` must be non-empty.

### health_check (`HttpFilterSpec::HealthCheck` → `HealthCheckConfig`)

| Field | Type | Required | Meaning |
|---|---|---|---|
| `endpoint_path` | `String` | required | Path the proxy answers itself (exact match), e.g. `/healthz`. |
| `pass_through_mode` | `bool` | optional (default `false`) | Pass health checks to the upstream instead of answering locally. |
| `cache_time_ms` | `Option<u64>` | optional | Cache time for pass-through responses. |

Validation:
- `endpoint_path` must start with `/` and be <= 500 characters.

Note: `health_check` is the only filter that is **not disablable** per-route (`is_disablable` returns `false`); it is listener-only.

### compressor (`HttpFilterSpec::Compressor` → `CompressorConfig`)

gzip compressor.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `memory_level` | `Option<u32>` | optional | zlib memory level, 1-9 (Envoy default 5 when omitted). |
| `window_bits` | `Option<u32>` | optional | zlib window bits, 9-15 (Envoy default 12 when omitted). |
| `compression_level` | `Option<CompressionLevel>` | optional | Compression level. |

`CompressionLevel` (`snake_case`): `best_speed`, `default_compression`, `best_compression`.

Validation:
- `memory_level`, if present, must be 1-9.
- `window_bits`, if present, must be 9-15.

### jwt_auth (`HttpFilterSpec::JwtAuth` → `JwtAuthConfig`)

| Field | Type | Required | Meaning |
|---|---|---|---|
| `providers` | `BTreeMap<String, JwtProvider>` | required | Provider name → provider (deterministic encoding). |
| `requirement_map` | `BTreeMap<String, JwtRequirement>` | optional (default empty) | Named requirements referenced by rules and per-route overrides. |
| `rules` | `Vec<JwtRule>` | optional (default empty) | Path rules, first match wins. Empty → every path requires any provider. |
| `bypass_cors_preflight` | `bool` | optional (default `false`) | Bypass JWT for CORS preflight requests. |

`JwtProvider`:

| Field | Type | Required | Meaning |
|---|---|---|---|
| `issuer` | `Option<String>` | optional | Expected token issuer. |
| `audiences` | `Vec<String>` | optional (default empty) | Accepted audiences. |
| `jwks` | `JwksSource` | required | Where the JWKS comes from. |
| `clock_skew_seconds` | `u32` | optional (default 60) | Clock skew tolerated for exp/nbf. |
| `forward` | `bool` | optional (default `false`) | Keep the token on the forwarded request (default: stripped). |

`JwksSource` (tagged by `source`, `snake_case`):
- `remote { uri, cluster, timeout_ms, cache_duration_secs }` — `uri` (full JWKS URI), `cluster` (same-team cluster used to reach the JWKS host), `timeout_ms` (`u64`, default 5000), `cache_duration_secs` (`Option<u64>`, optional).
- `inline { jwks }` — `jwks` (JWKS JSON inline).

`JwtRequirement` (tagged by `kind`, `snake_case`):
- `provider { provider_name }` — a specific provider must validate.
- `any_of { provider_names }` — any of the named providers validates.
- `allow_missing` — token optional; if present it must validate.
- `allow_missing_or_failed` — token optional and failures tolerated (audit-only).

`JwtRule`:
- `match` (`PathMatch`, required) — path matcher (field renamed to `match`).
- `requirement_name` (`String`, required) — name into `requirement_map`.

Validation:
- At least one provider is required.
- Each provider name passes `identity::validate_name`.
- Remote JWKS: `uri` must start with `http://` or `https://`; `cluster` passes `validate_name`; `timeout_ms` must be 1..=60000.
- Inline JWKS: `jwks` must be 1..=65536 bytes.
- Each `requirement_map` key passes `validate_name`; `any_of` must name at least one provider; requirements may not reference unknown providers.
- Each rule's `requirement_name` must exist in `requirement_map`.

### ext_authz (`HttpFilterSpec::ExtAuthz` → `ExtAuthzConfig`)

| Field | Type | Required | Meaning |
|---|---|---|---|
| `cluster` | `String` | required | gRPC authorization service, by same-team cluster name. |
| `timeout_ms` | `u64` | optional (default 200) | Authorization call timeout. |
| `failure_mode_allow` | `bool` | optional (default `false`) | Allow traffic when the authz service is unreachable; default `false` = fail closed. |
| `include_peer_certificate` | `bool` | optional (default `false`) | Include the peer certificate in the check request. |

Validation:
- `cluster` passes `identity::validate_name`.
- `timeout_ms` must be 1..=60000.

### rbac (`HttpFilterSpec::Rbac` → `RbacConfig`)

| Field | Type | Required | Meaning |
|---|---|---|---|
| `action` | `RbacAction` | required | `allow` or `deny`. |
| `policies` | `BTreeMap<String, RbacPolicy>` | required | Policy name → policy (deterministic encoding). |

`RbacAction` (`snake_case`): `allow` (matching requests allowed, rest denied), `deny` (matching requests denied, rest allowed).

`RbacPolicy`:
- `permissions` (`Vec<RbacPermission>`, required)
- `principals` (`Vec<RbacPrincipal>`, required)

`RbacPermission` (tagged by `kind`, `snake_case`):
- `any`
- `header { name, exact }` — `name` (String, required), `exact` (`Option<String>`, optional).
- `url_path { prefix }` — `prefix` (String, required).
- `destination_port { port }` — `port` (`u16`, required).

`RbacPrincipal` (tagged by `kind`, `snake_case`):
- `any`
- `source_cidr { cidr }` — `cidr` (String, required), direct peer address in CIDR form.
- `header { name, exact }` — `name` (String, required), `exact` (String, required).

Validation:
- At least one policy is required.
- Each policy name passes `identity::validate_name`.
- Each policy's `permissions` and `principals` must both be non-empty.
- `header` permission/principal: `name` must be non-empty.
- `url_path` permission: `prefix` must start with `/`.
- `destination_port` permission: `port` must be >= 1.
- `source_cidr` principal: `cidr` must be a valid CIDR (IPv4 prefix len <= 32, IPv6 prefix len <= 128).

### global_rate_limit (`HttpFilterSpec::GlobalRateLimit` → `GlobalRateLimitConfig`)

| Field | Type | Required | Meaning |
|---|---|---|---|
| `domain` | `String` | required | Rate-limit domain. |
| `service_cluster` | `String` | required | Rate-limit service (RLS) cluster. |
| `timeout_ms` | `u64` | optional (default 20) | RLS call timeout. |
| `failure_mode_deny` | `bool` | optional (default `false`) | Deny on RLS error. |
| `stage` | `u32` | optional (default 0) | Rate-limit stage. |
| `request_type` | `RateLimitRequestType` | optional (default `both`) | Which traffic the filter applies to. |
| `stat_prefix` | `Option<String>` | optional | Stats prefix. |
| `enable_x_ratelimit_headers` | `bool` | optional (default `false`) | Emit `x-ratelimit-*` headers (draft v03). |
| `disable_x_envoy_ratelimited_header` | `bool` | optional (default `false`) | Suppress `x-envoy-ratelimited`. |
| `rate_limited_status` | `Option<u16>` | optional | Status returned when rate-limited. |
| `status_on_error` | `Option<u16>` | optional | Status returned on RLS error. |

`RateLimitRequestType` (`snake_case`): `both` (default), `internal`, `external`.

Validation:
- `domain` must be 1..=128 characters and contain no NUL.
- `service_cluster` passes `identity::validate_name`.
- `timeout_ms` must be <= 60000.
- `stage` must be in 0..=10.
- `stat_prefix`, if present, must be 1..=128 characters and contain no NUL.
- `rate_limited_status` and `status_on_error`, if present, must be in 400..=599.

## Envoy filter name mapping

Domain kind → Envoy filter name URI. For the declared chain, the proto type URL/name (where it differs) is noted.

| Domain kind | Envoy filter name | Proto note |
|---|---|---|
| `cors` | `envoy.filters.http.cors` | chain entry is empty `Cors`; policy via per-route `CorsPolicy`. |
| `local_rate_limit` | `envoy.filters.http.local_ratelimit` | `LocalRateLimit` (same type URL in chain and per-route). |
| `header_mutation` | `envoy.filters.http.header_mutation` | `HeaderMutation`. |
| `compressor` | `envoy.filters.http.compressor` | `Compressor` (gzip library). |
| `health_check` | `envoy.filters.http.health_check` | `HealthCheck`. |
| `jwt_auth` | `envoy.filters.http.jwt_authn` | `JwtAuthentication`. |
| `ext_authz` | `envoy.filters.http.ext_authz` | `ExtAuthz`. |
| `rbac` | `envoy.filters.http.rbac` | type URL message name is `RBAC` (all-caps). |
| `global_rate_limit` | `envoy.filters.http.ratelimit` | `RateLimit`. |

Note: `envoy_filter_name()` (used for per-route `Disable` overrides) recognizes 8 kinds — it does **not** map `global_rate_limit`; that name (`envoy.filters.http.ratelimit`) is assigned directly in `http_filter_to_proto`. Any other kind passed to `envoy_filter_name()` returns `unknown filter type "…"`.

## Override scopes and per-scope overrides

Overrides are expressed with the `FilterOverride` enum (tagged by `type`, `snake_case`) and emitted as Envoy `typed_per_filter_config` at these scopes:

- **Listener chain** — `disabled` flag on the `HttpFilterEntry` (skips the filter for the whole listener).
- **Virtual host** — `filter_overrides` on the vhost → vhost-level `typed_per_filter_config`.
- **Route** — `filter_overrides` on the route → route-level `typed_per_filter_config`.

Per-scope rule (`validate_filter_overrides`): each override must be valid, and **at most one override may target a given filter type per scope**; a duplicate target yields `multiple overrides target filter type "…" in the same scope`.

`FilterOverride` variants and their target filter type (`target_kind`):

| Variant | Targets | Notes |
|---|---|---|
| `disable { filter_type }` | the named kind | Skip a chain filter on this scope. `filter_type` is a `kind()` string. Domain validation accepts every kind except `health_check` (an unknown or non-disablable type is rejected: `filter type "…" cannot be disabled per-route`). **Caveat:** `global_rate_limit` passes domain validation but currently **fails at xDS translation** — `envoy_filter_name()` maps only the other 8 kinds, so a `disable` targeting `global_rate_limit` errors with `unknown filter type "global_rate_limit"`. Effectively disablable kinds: `cors`, `local_rate_limit`, `header_mutation`, `compressor`, `jwt_auth`, `ext_authz`, `rbac`. |
| `cors { … CorsConfig }` | `cors` | CORS policy for this scope (requires the `cors` marker in the listener chain). |
| `local_rate_limit { … LocalRateLimitConfig }` | `local_rate_limit` | Replace the local rate limit on this scope. |
| `jwt_auth { requirement_name }` | `jwt_auth` | Reference-only: names a requirement from the chain filter's `requirement_map`. `requirement_name` must be 1..=128 characters. |

Only `cors`, `local_rate_limit`, and `jwt_auth` have dedicated per-scope config overrides. `disable` is accepted by domain validation for every kind except `health_check`, but is only translatable for the 7 kinds listed above (a `global_rate_limit` disable passes validation yet fails at xDS translation).

## Injected filters (not user-declared)

These are appended by the translator (`listener_to_proto_with_learning_and_ai`), never declared in the chain. Assembly order of the final HTTP filter list:

1. All declared chain filters, in declared order.
2. **AI ExtProc** (`envoy.filters.http.ext_proc.flowplane_ai`) — injected only when AI processor metadata is present. `ExternalProcessor` with `failure_mode_allow: false` (**fail closed / deny on processor failure**).
3. **Learning ExtProc** — one per capture, named `envoy.filters.http.ext_proc.flowplane_learning.<session_id>`. `ExternalProcessor` with `failure_mode_allow: true` (**fail open / allow on processor failure**), `is_optional: true`.
4. **Router** (`envoy.filters.http.router`, `Router`) — **always appended last**.

(An additional AI ExtProc, `…flowplane_ai.upstream`, is injected into upstream `HttpProtocolOptions` for AI clusters, also `failure_mode_allow: false`. It is not part of the listener HTTP filter chain.)

## Source of truth

- `crates/fp-domain/src/gateway/filters.rs` — filter vocabulary, config structs, validation, `FilterOverride`, and `validate_filter_overrides` / `validate_filter_chain`.
- `crates/fp-xds/src/translate.rs` — Envoy proto mapping (`envoy_filter_name`, `http_filter_to_proto`), chain assembly order, and injected filters.
