# Cluster Cookbook

Clusters describe the upstream services Envoy talks to. This cookbook highlights common configurations you can post to `/api/v1/clusters`. All payloads use camelCase fields (the API converts them to Envoy protos). Consult the live schema at `http://127.0.0.1:8080/swagger-ui` for request/response definitions.

## 1. Basic HTTP Cluster
The simplest cluster definition points Envoy at one or more HTTP endpoints.

```bash
curl -sS -X POST http://127.0.0.1:8080/api/v1/clusters \
  -H 'Content-Type: application/json' \
  -d '{
    "name": "catalog-cluster",
    "serviceName": "catalog",
    "endpoints": [
      {"host": "catalog.example.com", "port": 8080}
    ],
    "connectTimeoutSeconds": 3
  }'
```

Use this for plain HTTP backends or as a starting point before enabling TLS and health checks.

## 2. TLS-Enabled Upstream
Enable TLS to reach HTTPS services and optionally set SNI.

```bash
curl -sS -X POST http://127.0.0.1:8080/api/v1/clusters \
  -H 'Content-Type: application/json' \
  -d '{
    "name": "payments-cluster",
    "serviceName": "payments",
    "endpoints": [{"host": "payments.internal", "port": 8443}],
    "useTls": true,
    "tlsServerName": "payments.internal",
    "connectTimeoutSeconds": 5
  }'
```

Ideal when calling SaaS APIs or internal services behind TLS. Pair with static secrets if mutual TLS is required (see listener cookbook for TLS context examples).

## 3. Health-Checked Cluster
Add active health checks so Envoy only routes to healthy endpoints.

```bash
curl -sS -X POST http://127.0.0.1:8080/api/v1/clusters \
  -H 'Content-Type: application/json' \
  -d '{
    "name": "inventory-cluster",
    "endpoints": [
      {"host": "inventory-a", "port": 8080},
      {"host": "inventory-b", "port": 8080}
    ],
    "healthChecks": [
      {
        "type": "http",
        "path": "/health",
        "intervalSeconds": 10,
        "timeoutSeconds": 3,
        "healthyThreshold": 2,
        "unhealthyThreshold": 3
      }
    ]
  }'
```

Use HTTP health checks for REST services; switch `type` to `tcp` or `grpc` for other protocols.

## 4. Circuit Breakers and Outlier Detection
Protect upstreams from overload and automatically eject unhealthy hosts.

```bash
curl -sS -X POST http://127.0.0.1:8080/api/v1/clusters \
  -H 'Content-Type: application/json' \
  -d '{
    "name": "orders-cluster",
    "endpoints": [{"host": "orders", "port": 9000}],
    "circuitBreakers": {
      "default": {
        "maxConnections": 200,
        "maxPendingRequests": 100,
        "maxRequests": 500,
        "maxRetries": 3
      }
    },
    "outlierDetection": {
      "consecutive5xx": 5,
      "intervalSeconds": 30,
      "baseEjectionTimeSeconds": 60,
      "maxEjectionPercent": 50
    }
  }'
```

Circuit breakers limit concurrent load; outlier detection removes consistently failing endpoints.

## 5. Alternative Load Balancing Policies
Switch to least-request or consistent hashing depending on the workload.

```bash
curl -sS -X POST http://127.0.0.1:8080/api/v1/clusters \
  -H 'Content-Type: application/json' \
  -d '{
    "name": "profile-cluster",
    "endpoints": [
      {"host": "profile-a", "port": 8080},
      {"host": "profile-b", "port": 8080}
    ],
    "lbPolicy": "LEAST_REQUEST"
  }'
```

Other options: `RING_HASH` (stable hashing), `MAGLEV` (consistent hashing with better distribution), or `RANDOM`.

## 6. DNS Service Discovery
Let Envoy resolve endpoints dynamically via DNS.

```bash
curl -sS -X POST http://127.0.0.1:8080/api/v1/clusters \
  -H 'Content-Type: application/json' \
  -d '{
    "name": "search-cluster",
    "serviceName": "search.internal",
    "dnsLookupFamily": "AUTO",
    "type": "STRICT_DNS",
    "endpoints": [
      {"host": "search.internal", "port": 8080}
    ]
  }'
```

Use `STRICT_DNS` (default) for periodically refreshed DNS lookups; `LOGICAL_DNS` keeps a single address in rotation.

## Operations
- List clusters: `GET /api/v1/clusters`
- Get details: `GET /api/v1/clusters/{name}`
- Update: `PUT /api/v1/clusters/{name}` (same payload shape)
- Delete: `DELETE /api/v1/clusters/{name}`

Payloads are validated server-side; errors return HTTP 400 with actionable messages. Combine these recipes with the [routing](routing-cookbook.md) and [listener](listener-cookbook.md) guides to build end-to-end gateways.
