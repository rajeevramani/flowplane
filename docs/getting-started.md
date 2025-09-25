# Getting Started

This walkthrough takes you from an empty database to a working Envoy listener that enforces global and per-route rate limits. All API calls use `curl`, and the examples assume the control plane is running on `http://127.0.0.1:8080` (see the [README](../README.md) for the launch command).

> **New:** Already have an OpenAPI 3.0 spec? Call `POST /api/v1/gateways/openapi?name=<gateway>` with your JSON or YAML document to generate clusters, routes, and a listener automatically. You can still follow the manual steps below to fine-tune or extend the generated resources.

## 1. Explore the API Reference
Open `http://127.0.0.1:8080/swagger-ui` in your browser. The Swagger UI lists every endpoint, schema, and example. You can execute requests directly from the UI or copy the `curl` commands shown below.

## 2. Register a Cluster
Clusters describe upstream backends. This example creates a TLS-enabled cluster that forwards to `httpbin.org`:

```bash
curl -sS -X POST http://127.0.0.1:8080/api/v1/clusters \
  -H 'Content-Type: application/json' \
  -d '{
    "name": "demo-cluster",
    "serviceName": "httpbin",
    "endpoints": [
      { "host": "httpbin.org", "port": 443 }
    ],
    "connectTimeoutSeconds": 5,
    "useTls": true,
    "tlsServerName": "httpbin.org"
  }'
```

*Response*: `201 Created` with the stored cluster definition. Use `GET /api/v1/clusters/demo-cluster` to verify later.

## 3. Publish a Route Configuration
Routes map request prefixes to clusters. Here we forward everything under `/` to `demo-cluster` and apply a per-route Local Rate Limit (20 requests/second, returning HTTP 429 when exhausted).

> **Snake case fields** – Filter-specific blocks (like Local Rate Limit) use snake_case (`stat_prefix`, `token_bucket`). The REST layer handles the conversion to Envoy protos for you.

```bash
curl -sS -X POST http://127.0.0.1:8080/api/v1/routes \
  -H 'Content-Type: application/json' \
  -d '{
    "name": "demo-routes",
    "virtualHosts": [
      {
        "name": "default",
        "domains": ["*"],
        "routes": [
          {
            "name": "to-httpbin",
            "match": {
              "path": {"type": "prefix", "value": "/"}
            },
            "action": {
              "type": "forward",
              "cluster": "demo-cluster",
              "timeoutSeconds": 10
            },
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
          }
        ]
      }
    ]
  }'
```

## 4. Create a Listener with Global Filters
Listeners bind ports and assemble filter chains. This example:

1. Adds a listener-wide Local Rate Limit (100 requests/second).
2. Enables Envoy’s router filter (appended automatically if omitted, but included here for clarity).
3. Points the HTTP connection manager at `demo-routes`.

```bash
curl -sS -X POST http://127.0.0.1:8080/api/v1/listeners \
  -H 'Content-Type: application/json' \
  -d '{
    "name": "demo-listener",
    "address": "0.0.0.0",
    "port": 10000,
    "protocol": "HTTP",
    "filterChains": [
      {
        "name": "default",
        "filters": [
          {
            "name": "envoy.filters.network.http_connection_manager",
            "type": "httpConnectionManager",
            "routeConfigName": "demo-routes",
            "httpFilters": [
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
              },
              {
                "name": "envoy.filters.http.router",
                "filter": {
                  "type": "router"
                }
              }
            ]
          }
        ]
      }
    ]
  }'
```

> **Want JWT authentication?** See [docs/filters.md](filters.md#jwt-authentication) for the provider and requirement fields. You can add the `jwt_authn` filter into the same `httpFilters` list before the router entry.

## 5. Point Envoy at the Control Plane
Configure an Envoy bootstrap with ADS pointing at `127.0.0.1:18003` (the `FLOWPLANE_XDS_PORT` value). Once Envoy connects, it will receive the cluster, route, and listener we created.

Minimal snippet:

```yaml
ads_config:
  api_type: GRPC
  transport_api_version: V3
  grpc_services:
    - envoy_grpc:
        cluster_name: xds_cluster
static_resources:
  clusters:
    - name: xds_cluster
      connect_timeout: 1s
      type: STATIC
      http2_protocol_options: {}
      load_assignment:
        cluster_name: xds_cluster
        endpoints:
          - lb_endpoints:
              - endpoint:
                  address:
                    socket_address:
                      address: 127.0.0.1
                      port_value: 18003
```

(See Envoy’s documentation for a full bootstrap; this example focuses on the XDS connection.)

## 6. Verify Traffic
With Envoy running and Flowplane serving resources, send a request through the listener:

```bash
curl -i http://127.0.0.1:10000/status/200
```

Repeated requests will eventually trigger either the listener-wide or per-route rate limit and return `429 Too Many Requests` with headers indicating the rate limit action.

## Next Steps
- Explore cluster variations (TLS, health checks, circuit breakers) in the [cluster cookbook](cluster-cookbook.md).
- Try advanced routing patterns (weighted splits, redirects, scoped filters) in the [routing cookbook](routing-cookbook.md).
- Configure listener features (JWT auth, global rate limits, TLS termination) in the [listener cookbook](listener-cookbook.md).
- Assemble end-to-end gateway scenarios with the [API gateway recipes](gateway-recipes.md).
- Dive into filter details in [filters.md](filters.md) and explore scoped overrides.
- Use `GET /api/v1/*` endpoints to inspect stored resources, and `DELETE` to remove them.
- Run `scripts/smoke-listener.sh` for an automated sanity check.

Once comfortable with the basics, dive into the [architecture overview](architecture.md) and start planning contributions.
