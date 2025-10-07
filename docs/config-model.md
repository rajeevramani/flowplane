# Configuration Model

Flowplane exposes REST resources that map cleanly onto Envoy listener, route, and cluster protos. All payloads use camelCase for resource fields and snake_case for filter-specific blocks, validated before translation to Envoy protobufs.

## API Access

**All endpoints require bearer authentication:**

```bash
export FLOWPLANE_TOKEN="fp_pat_..."
curl -H "Authorization: Bearer $FLOWPLANE_TOKEN" http://127.0.0.1:8080/api/v1/clusters
```

See [authentication.md](authentication.md) for token management and scopes.

**Interactive Documentation:**
- **Swagger UI**: `http://127.0.0.1:8080/swagger-ui` - Test endpoints, view schemas
- **OpenAPI JSON**: `http://127.0.0.1:8080/api-docs/openapi.json` - Machine-readable spec

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

Endpoint: `POST /api/v1/clusters`

Clusters define upstream services and their endpoints. Flowplane translates cluster configurations into Envoy `Cluster` resources with full support for load balancing, health checks, circuit breakers, and TLS.

### Required Fields

| Field | Description |
| ----- | ----------- |
| `name` | Unique cluster identifier |
| `serviceName` | Service name for service discovery |
| `endpoints` | Array of `{host, port}` endpoint definitions |
| `connectTimeoutSeconds` | Connection timeout in seconds |

### Optional Fields

| Field | Description |
| ----- | ----------- |
| `type` | Cluster type: `STATIC`, `STRICT_DNS`, `LOGICAL_DNS` (default: `STATIC`) |
| `lbPolicy` | Load balancing: `ROUND_ROBIN`, `LEAST_REQUEST`, `RING_HASH`, `RANDOM` (default: `ROUND_ROBIN`) |
| `useTls` | Enable TLS for upstream connections (default: `false`) |
| `tlsServerName` | SNI hostname for TLS validation |
| `healthCheck` | Health check configuration (see below) |
| `circuitBreakers` | Circuit breaker thresholds |
| `outlierDetection` | Outlier detection configuration |

### Health Check Configuration

```json
{
  "healthCheck": {
    "path": "/health",
    "intervalSeconds": 10,
    "timeoutSeconds": 2,
    "unhealthyThreshold": 3,
    "healthyThreshold": 2,
    "expectedStatuses": [200, 204]
  }
}
```

### Circuit Breaker Configuration

```json
{
  "circuitBreakers": {
    "maxConnections": 1024,
    "maxPendingRequests": 1024,
    "maxRequests": 1024,
    "maxRetries": 3
  }
}
```

### Complete Example

```bash
curl -sS -X POST http://127.0.0.1:8080/api/v1/clusters \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "backend-service",
    "serviceName": "backend",
    "type": "STRICT_DNS",
    "lbPolicy": "ROUND_ROBIN",
    "endpoints": [
      {"host": "backend1.example.com", "port": 443},
      {"host": "backend2.example.com", "port": 443}
    ],
    "connectTimeoutSeconds": 5,
    "useTls": true,
    "tlsServerName": "backend.example.com",
    "healthCheck": {
      "path": "/healthz",
      "intervalSeconds": 30,
      "timeoutSeconds": 5,
      "unhealthyThreshold": 3,
      "healthyThreshold": 2
    },
    "circuitBreakers": {
      "maxConnections": 1000,
      "maxRequests": 1000,
      "maxRetries": 5
    }
  }'
```

See [cluster-cookbook.md](cluster-cookbook.md) for advanced patterns (DNS-based discovery, outlier detection, TLS mutual authentication).

## Typed Config Payloads
Some Envoy features still require arbitrary protobuf payloads. Use `TypedConfig` `{ "typeUrl": "...", "value": "<base64>" }` wherever a raw `Any` is needed. The helper structs in `src/xds/filters` simplify this for filters, but the escape hatch remains available.

## Platform API (API Definitions)

Endpoint: `POST /api/v1/api-definitions/from-openapi`

The Platform API provides higher-level abstractions for gateway creation. Instead of manually crafting clusters, routes, and listeners, import an OpenAPI 3.0 specification and Flowplane generates all resources automatically.

### OpenAPI Import

```bash
curl -sS \
  -X POST "http://127.0.0.1:8080/api/v1/api-definitions/from-openapi?team=myteam" \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  -H "Content-Type: application/json" \
  --data-binary @openapi.json
```

**Query Parameters:**

| Parameter | Required | Default | Description |
|-----------|----------|---------|-------------|
| `team` | Yes | - | Team namespace for resources |
| `listener` | No | `default-gateway-listener` | Listener name (creates new if custom) |
| `port` | No | `10000` | Listener port (only with custom listener) |
| `bind_address` | No | `0.0.0.0` | Bind address (only with custom listener) |
| `protocol` | No | `HTTP` | Protocol (only with custom listener) |

**Generated Resources:**
- **Clusters**: One per `servers[].url` in OpenAPI spec
- **Routes**: One per `paths` entry with HTTP method filtering
- **Listener**: Shared (`default-gateway-listener`) or dedicated based on query params

### Multi-Tenancy

**Shared Listener Mode (Default):**
- All teams join `default-gateway-listener` on port `10000`
- Cost-efficient, zero configuration
- Routes namespaced by path prefix or domain

**Dedicated Listener Mode:**
- Specify `?listener=myteam-gateway&port=8080`
- Full isolation with independent filter configuration
- Per-team rate limits, JWT providers, CORS policies

### OpenAPI Extensions

Flowplane supports `x-flowplane-filters` and `x-flowplane-custom-response` extensions for filter configuration:

```yaml
x-flowplane-filters:
  cors:
    allow_origin: ["*"]
    allow_methods: ["GET", "POST"]
  local_rate_limit:
    stat_prefix: "api_ratelimit"
    token_bucket:
      max_tokens: 100
      tokens_per_fill: 100
      fill_interval_ms: 1000

paths:
  /api/users:
    get:
      x-flowplane-custom-response:
        status_code: 429
        body: "Rate limit exceeded"
```

See [examples/README-x-flowplane-extensions.md](../examples/README-x-flowplane-extensions.md) for complete reference.

## Environment Variables

Flowplane is configured via environment variables at startup. See [README.md](../README.md#environment-variables-reference) for complete reference.

### Core Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `DATABASE_URL` | *required* | SQLite or PostgreSQL connection string |
| `FLOWPLANE_API_BIND_ADDRESS` | `127.0.0.1` | API server bind address |
| `FLOWPLANE_API_PORT` | `8080` | API server port |
| `FLOWPLANE_XDS_BIND_ADDRESS` | `0.0.0.0` | xDS server bind address |
| `FLOWPLANE_XDS_PORT` | `50051` | xDS gRPC server port |

### TLS Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `FLOWPLANE_API_TLS_ENABLED` | `false` | Enable HTTPS for API |
| `FLOWPLANE_API_TLS_CERT_PATH` | - | API server certificate path |
| `FLOWPLANE_API_TLS_KEY_PATH` | - | API server private key path |
| `FLOWPLANE_XDS_TLS_CERT_PATH` | - | xDS server certificate path |
| `FLOWPLANE_XDS_TLS_KEY_PATH` | - | xDS server private key path |
| `FLOWPLANE_XDS_TLS_CLIENT_CA_PATH` | - | CA bundle for client cert validation |

### Observability

| Variable | Default | Description |
|----------|---------|-------------|
| `RUST_LOG` | `info` | Logging level (`error`, `warn`, `info`, `debug`, `trace`) |
| `FLOWPLANE_ENABLE_METRICS` | `true` | Enable Prometheus metrics |
| `FLOWPLANE_ENABLE_TRACING` | `false` | Enable OpenTelemetry tracing |

## TLS Configuration

Flowplane supports TLS termination for both the API server and xDS server.

### API TLS (HTTPS)

Enable HTTPS for the REST API:

```bash
FLOWPLANE_API_TLS_ENABLED=true \
FLOWPLANE_API_TLS_CERT_PATH=/path/to/api-cert.pem \
FLOWPLANE_API_TLS_KEY_PATH=/path/to/api-key.pem \
FLOWPLANE_API_TLS_CHAIN_PATH=/path/to/chain.pem \
cargo run --bin flowplane
```

**Certificate Requirements:**
- PEM-encoded leaf certificate
- PEM-encoded private key matching certificate
- Optional intermediate chain for client validation

### xDS TLS/mTLS

Secure control plane ↔ data plane communication:

```bash
FLOWPLANE_XDS_TLS_CERT_PATH=/path/to/xds-server.pem \
FLOWPLANE_XDS_TLS_KEY_PATH=/path/to/xds-server.key \
FLOWPLANE_XDS_TLS_CLIENT_CA_PATH=/path/to/xds-ca.pem \
FLOWPLANE_XDS_TLS_REQUIRE_CLIENT_CERT=true \
cargo run --bin flowplane
```

**Mutual TLS:**
- `FLOWPLANE_XDS_TLS_CLIENT_CA_PATH` enables client certificate validation
- `FLOWPLANE_XDS_TLS_REQUIRE_CLIENT_CERT=true` enforces mTLS (default)
- Set to `false` for one-way TLS (server auth only)

**Envoy Configuration:**

Envoy must present a valid client certificate when `REQUIRE_CLIENT_CERT=true`:

```yaml
static_resources:
  clusters:
    - name: xds_cluster
      transport_socket:
        name: envoy.transport_sockets.tls
        typed_config:
          "@type": type.googleapis.com/envoy.extensions.transport_sockets.tls.v3.UpstreamTlsContext
          sni: flowplane-control-plane
          common_tls_context:
            tls_certificates:
              - certificate_chain: {filename: /etc/envoy/certs/xds-client.pem}
                private_key: {filename: /etc/envoy/certs/xds-client.key}
            validation_context:
              trusted_ca: {filename: /etc/envoy/certs/xds-ca.pem}
              match_subject_alt_names:
                - exact: flowplane-control-plane
```

See [tls.md](tls.md) for certificate generation workflows (ACME, corporate PKI, development).

## Validation

The server validates:

* **Required fields** - E.g., HTTP connection manager must specify route config source
* **Filter invariants** - Router uniqueness, rate limit bucket requirements, JWT provider keys
* **Name formats** - Must match `VALID_NAME_REGEX` (alphanumeric, hyphens, underscores)
* **Token scopes** - Requests must present valid bearer token with appropriate scopes
* **Schema compliance** - All payloads validated against OpenAPI schema

Requests failing validation receive `400 Bad Request` with descriptive error messages:

```json
{
  "error": "validation_failed",
  "message": "Invalid cluster configuration",
  "details": [
    "endpoints: must contain at least one endpoint",
    "connectTimeoutSeconds: must be positive"
  ]
}
```

## Additional Resources

- **Getting Started**: [getting-started.md](getting-started.md) - Step-by-step tutorial
- **API Reference**: [api.md](api.md) - Complete endpoint documentation
- **Filters**: [filters.md](filters.md) - HTTP filter configuration
- **Architecture**: [architecture.md](architecture.md) - System design and components
- **Cluster Patterns**: [cluster-cookbook.md](cluster-cookbook.md) - Advanced cluster configurations
- **Routing Patterns**: [routing-cookbook.md](routing-cookbook.md) - Complex routing scenarios
- **Interactive Docs**: http://127.0.0.1:8080/swagger-ui - Live API exploration
