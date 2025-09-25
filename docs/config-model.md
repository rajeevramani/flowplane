# Configuration Model

Flowplane exposes REST resources that map cleanly onto Envoy listener, route, and cluster protos. All payloads accept camelCase field names (with filter blocks in snake_case) and are validated before translation. Browse the live schema at `http://127.0.0.1:8080/swagger-ui` or fetch the raw OpenAPI document from `/api-docs/openapi.json`.

## Listeners
Endpoint: `POST /api/v1/listeners`

Key fields:

| Field | Description |
| ----- | ----------- |
| `name` | Unique identifier for the listener resource. |
| `address` / `port` | Bind address/port for Envoy’s socket. |
| `filterChains` | Array of filter chains; each includes `filters`, optional `tlsContext`. |

`FilterType::HttpConnectionManager` drives HTTP listeners. Provide either `routeConfigName` (for ADS/RDS) or `inlineRouteConfig`. Optional components:

* `accessLog` – file path + format string.
* `tracing` – provider name + arbitrary string map.
* `httpFilters` – ordered list of `HttpFilterConfigEntry` items (see [filters](filters.md)). The router filter is appended automatically if you omit it.

## Routes
Endpoint: `POST /api/v1/routes`

Structure:

* `RouteConfig` – name + list of `VirtualHostConfig` entries.
* `VirtualHostConfig` – domains, routes, optional `typedPerFilterConfig` for scoped overrides at the host level.
* `RouteRule` – match + action + optional per-filter configs.

`RouteMatchConfig` currently supports exact/prefix/regex/template path matching. Header and query-parameter matchers will be added once the translation layer wires them into Envoy resources. Route actions include:

* `Cluster` – direct cluster reference (with optional timeout, prefix/path rewrites).
* `WeightedClusters` – traffic split with optional filter configs per weight.
* `Redirect` – host/path redirect with optional status code.

Attach HTTP filter overrides through `typedPerFilterConfig`, e.g.

```json
"typedPerFilterConfig": {
  "envoy.filters.http.jwt_authn": {
    "jwtAuthn": { "requirementName": "allow_optional" }
  }
}
```

## Clusters
Clusters are registered through the API (not shown in detail here) and include standard fields:

* `name`, `connectTimeout`, `type`, `lbPolicy`, and endpoint definitions.
* Optional circuit breakers, outlier detection, health checks, TLS contexts.

Refer to the OpenAPI schema generated via `utoipa` or inspect `src/api` handlers for full details.

## Typed Config Payloads
Some Envoy features still require arbitrary protobuf payloads. Use `TypedConfig` `{ "typeUrl": "...", "value": "<base64>" }` wherever a raw `Any` is needed. The helper structs in `src/xds/filters` simplify this for filters, but the escape hatch remains available.

## Validation
The server validates:

* Required fields (e.g., HTTP connection manager must specify a route config source).
* Filter-specific invariants (router uniqueness, Local Rate Limit bucket requirements, JWT provider metadata keys).
* Name formats (see `utils::VALID_NAME_REGEX`).

Requests failing validation receive `400` responses with descriptive error messages.
