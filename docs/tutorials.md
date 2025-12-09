# Step-by-Step Tutorials

This guide provides complete, tested tutorials for common Flowplane use cases. Each tutorial is self-contained and includes verification steps.

## Tutorial 1: Basic API Gateway (10 minutes)

**Goal**: Create a simple HTTP gateway that proxies requests to `httpbin.org`

**Prerequisites:**
- Flowplane running on `http://127.0.0.1:8080`
- Bootstrap token available (`$FLOWPLANE_TOKEN`)
- `curl` and `jq` installed

### Step 1: Create Backend Cluster

```bash
curl -sS -X POST http://127.0.0.1:8080/api/v1/clusters \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "httpbin-cluster",
    "serviceName": "httpbin",
    "endpoints": [
      {"host": "httpbin.org", "port": 443}
    ],
    "connectTimeoutSeconds": 5,
    "useTls": true,
    "tlsServerName": "httpbin.org"
  }' | jq

# Verify
curl -sS -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  http://127.0.0.1:8080/api/v1/clusters/httpbin-cluster | jq .name
```

**Expected output:** `"httpbin-cluster"`

### Step 2: Create Route Configuration

```bash
curl -sS -X POST http://127.0.0.1:8080/api/v1/route-configs \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "httpbin-routes",
    "virtualHosts": [{
      "name": "httpbin-host",
      "domains": ["*"],
      "routes": [{
        "name": "all-traffic",
        "match": {
          "path": {"type": "prefix", "value": "/"}
        },
        "action": {
          "type": "forward",
          "cluster": "httpbin-cluster",
          "timeoutSeconds": 30
        }
      }]
    }]
  }' | jq

# Verify
curl -sS -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  http://127.0.0.1:8080/api/v1/route-configs/httpbin-routes | jq .name
```

### Step 3: Create Listener

```bash
curl -sS -X POST http://127.0.0.1:8080/api/v1/listeners \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "httpbin-listener",
    "address": "0.0.0.0",
    "port": 10001,
    "protocol": "HTTP",
    "filterChains": [{
      "name": "default",
      "filters": [{
        "name": "envoy.filters.network.http_connection_manager",
        "type": "httpConnectionManager",
        "routeConfigName": "httpbin-routes",
        "httpFilters": [
          {"name": "envoy.filters.http.router", "filter": {"type": "router"}}
        ]
      }]
    }]
  }' | jq
```

### Step 4: Point Envoy at Control Plane

Create Envoy bootstrap config (`envoy-bootstrap.yaml`):

```yaml
admin:
  address:
    socket_address: {address: 0.0.0.0, port_value: 9901}

node:
  cluster: tutorial-cluster
  id: tutorial-node

dynamic_resources:
  ads_config:
    api_type: GRPC
    transport_api_version: V3
    grpc_services:
      - envoy_grpc:
          cluster_name: xds_cluster
  cds_config:
    resource_api_version: V3
    ads: {}
  lds_config:
    resource_api_version: V3
    ads: {}

static_resources:
  clusters:
    - name: xds_cluster
      connect_timeout: 1s
      type: STRICT_DNS
      http2_protocol_options: {}
      load_assignment:
        cluster_name: xds_cluster
        endpoints:
          - lb_endpoints:
              - endpoint:
                  address:
                    socket_address:
                      address: 127.0.0.1
                      port_value: 50051
```

Start Envoy:

```bash
envoy -c envoy-bootstrap.yaml
```

### Step 5: Test Gateway

```bash
# Test through Envoy (port 10001)
curl -i http://127.0.0.1:10001/get

# Expected: 200 OK with JSON response from httpbin.org
```

**Troubleshooting:**
- If 404: Check listener is created with correct port
- If connection refused: Verify Envoy is running and connected to xDS
- If 503: Check cluster endpoint is reachable

---

## Tutorial 2: Rate-Limited API Gateway (15 minutes)

**Goal**: Create gateway with global and per-route rate limiting

**Prerequisites:** Complete Tutorial 1 or have basic gateway running

### Step 1: Create Rate-Limited Listener

```bash
curl -sS -X POST http://127.0.0.1:8080/api/v1/listeners \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "ratelimited-listener",
    "address": "0.0.0.0",
    "port": 10002,
    "protocol": "HTTP",
    "filterChains": [{
      "name": "default",
      "filters": [{
        "name": "envoy.filters.network.http_connection_manager",
        "type": "httpConnectionManager",
        "routeConfigName": "httpbin-routes",
        "httpFilters": [
          {
            "name": "envoy.filters.http.local_ratelimit",
            "filter": {
              "type": "local_rate_limit",
              "stat_prefix": "global_ratelimit",
              "token_bucket": {
                "max_tokens": 100,
                "tokens_per_fill": 100,
                "fill_interval_ms": 1000
              },
              "status_code": 429
            }
          },
          {"name": "envoy.filters.http.router", "filter": {"type": "router"}}
        ]
      }]
    }]
  }' | jq
```

### Step 2: Create Route with Per-Route Rate Limit

```bash
curl -sS -X POST http://127.0.0.1:8080/api/v1/routes \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "ratelimited-routes",
    "virtualHosts": [{
      "name": "httpbin-host",
      "domains": ["*"],
      "routes": [
        {
          "name": "status-endpoint",
          "match": {
            "path": {"type": "prefix", "value": "/status"}
          },
          "action": {
            "type": "forward",
            "cluster": "httpbin-cluster"
          },
          "typedPerFilterConfig": {
            "envoy.filters.http.local_ratelimit": {
              "stat_prefix": "status_route",
              "token_bucket": {
                "max_tokens": 10,
                "tokens_per_fill": 10,
                "fill_interval_ms": 1000
              },
              "status_code": 429
            }
          }
        },
        {
          "name": "get-endpoint",
          "match": {
            "path": {"type": "prefix", "value": "/get"}
          },
          "action": {
            "type": "forward",
            "cluster": "httpbin-cluster"
          },
          "typedPerFilterConfig": {
            "envoy.filters.http.local_ratelimit": {
              "stat_prefix": "get_route",
              "token_bucket": {
                "max_tokens": 50,
                "tokens_per_fill": 50,
                "fill_interval_ms": 1000
              },
              "status_code": 429
            }
          }
        }
      ]
    }]
  }' | jq
```

### Step 3: Update Listener to Use New Routes

```bash
curl -sS -X PUT http://127.0.0.1:8080/api/v1/listeners/ratelimited-listener \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "ratelimited-listener",
    "address": "0.0.0.0",
    "port": 10002,
    "protocol": "HTTP",
    "filterChains": [{
      "name": "default",
      "filters": [{
        "name": "envoy.filters.network.http_connection_manager",
        "type": "httpConnectionManager",
        "routeConfigName": "ratelimited-routes",
        "httpFilters": [
          {
            "name": "envoy.filters.http.local_ratelimit",
            "filter": {
              "type": "local_rate_limit",
              "stat_prefix": "global_ratelimit",
              "token_bucket": {
                "max_tokens": 100,
                "tokens_per_fill": 100,
                "fill_interval_ms": 1000
              }
            }
          },
          {"name": "envoy.filters.http.router", "filter": {"type": "router"}}
        ]
      }]
    }]
  }' | jq
```

### Step 4: Test Rate Limits

```bash
# Test status endpoint (10 req/sec limit)
for i in {1..15}; do
  curl -s -o /dev/null -w "%{http_code}\n" http://127.0.0.1:10002/status/200
done

# Expected: First 10 return 200, remaining return 429

# Test get endpoint (50 req/sec limit)
for i in {1..60}; do
  curl -s -o /dev/null -w "%{http_code}\n" http://127.0.0.1:10002/get
done

# Expected: First 50 return 200, remaining return 429
```

**Verification:**
```bash
# Check Envoy stats
curl -s http://127.0.0.1:9901/stats | grep local_rate_limit
```

---

## Tutorial 3: CORS-Enabled API Gateway (10 minutes)

**Goal**: Configure CORS policy for web application access

### Step 1: Create CORS-Enabled Listener

```bash
curl -sS -X POST http://127.0.0.1:8080/api/v1/listeners \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "cors-listener",
    "address": "0.0.0.0",
    "port": 10003,
    "protocol": "HTTP",
    "filterChains": [{
      "name": "default",
      "filters": [{
        "name": "envoy.filters.network.http_connection_manager",
        "type": "httpConnectionManager",
        "routeConfigName": "httpbin-routes",
        "httpFilters": [
          {
            "name": "envoy.filters.http.cors",
            "filter": {
              "type": "cors",
              "allow_origin_string_match": [
                {"exact": "https://app.example.com"},
                {"prefix": "https://dev"},
                {"suffix": ".example.com"}
              ],
              "allow_methods": ["GET", "POST", "PUT", "DELETE", "OPTIONS"],
              "allow_headers": ["Content-Type", "Authorization", "X-Request-ID"],
              "expose_headers": ["X-RateLimit-Remaining", "X-RateLimit-Reset"],
              "max_age": "86400",
              "allow_credentials": true
            }
          },
          {"name": "envoy.filters.http.router", "filter": {"type": "router"}}
        ]
      }]
    }]
  }' | jq
```

### Step 2: Test CORS Preflight

```bash
# Test OPTIONS preflight request
curl -i -X OPTIONS http://127.0.0.1:10003/get \
  -H "Origin: https://app.example.com" \
  -H "Access-Control-Request-Method: POST" \
  -H "Access-Control-Request-Headers: Content-Type"

# Expected headers:
# Access-Control-Allow-Origin: https://app.example.com
# Access-Control-Allow-Methods: GET, POST, PUT, DELETE, OPTIONS
# Access-Control-Max-Age: 86400
```

### Step 3: Test Actual CORS Request

```bash
# Test GET request with Origin header
curl -i http://127.0.0.1:10003/get \
  -H "Origin: https://app.example.com"

# Expected: 200 OK with Access-Control-Allow-Origin header
```

---

## Tutorial 4: OpenAPI Import with Filters (15 minutes)

**Goal**: Import OpenAPI spec and apply filters automatically

### Step 1: Create OpenAPI Specification

Create `my-api.yaml`:

```yaml
openapi: 3.0.0
info:
  title: My API
  version: 1.0.0
  description: Example API with Flowplane extensions

servers:
  - url: https://api.example.com

x-flowplane-filters:
  cors:
    allow_origin_string_match:
      - exact: "https://app.example.com"
    allow_methods: ["GET", "POST", "PUT", "DELETE"]
    allow_headers: ["Content-Type", "Authorization"]
    max_age: "3600"
  local_rate_limit:
    stat_prefix: "api_global"
    token_bucket:
      max_tokens: 1000
      tokens_per_fill: 1000
      fill_interval_ms: 1000

paths:
  /users:
    get:
      summary: List users
      x-flowplane-custom-response:
        status_code: 429
        body: '{"error": "rate_limit_exceeded", "message": "Too many requests"}'
      responses:
        '200':
          description: Success
          content:
            application/json:
              schema:
                type: array
                items:
                  type: object
    post:
      summary: Create user
      responses:
        '201':
          description: Created

  /users/{id}:
    get:
      summary: Get user by ID
      parameters:
        - name: id
          in: path
          required: true
          schema:
            type: string
      responses:
        '200':
          description: Success
        '404':
          description: Not found
```

### Step 2: Import to Flowplane

```bash
curl -sS -X POST "http://127.0.0.1:8080/api/v1/api-definitions/from-openapi?team=myteam" \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  -H "Content-Type: application/yaml" \
  --data-binary @my-api.yaml | jq
```

### Step 3: Verify Resources Created

```bash
# Check cluster
curl -sS -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  http://127.0.0.1:8080/api/v1/clusters | jq '.[] | select(.name | contains("myteam"))'

# Check routes
curl -sS -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  http://127.0.0.1:8080/api/v1/routes | jq '.[] | select(.name | contains("myteam"))'

# Check listener (default-gateway-listener on port 10000)
curl -sS -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  http://127.0.0.1:8080/api/v1/listeners/default-gateway-listener | jq
```

### Step 4: Test API Gateway

```bash
# Test through default gateway listener
curl -i http://127.0.0.1:10000/users \
  -H "Host: api.example.com" \
  -H "Origin: https://app.example.com"

# Expected: 200 OK with CORS headers
```

---

## Tutorial 5: Multi-Team Gateway (20 minutes)

**Goal**: Set up isolated gateways for two teams sharing infrastructure

### Step 1: Team A - Shared Listener

```bash
# Team A imports to shared listener (default behavior)
cat > team-a-api.yaml <<EOF
openapi: 3.0.0
info:
  title: Team A API
  version: 1.0.0
servers:
  - url: https://team-a.example.com
paths:
  /api/v1/resources:
    get:
      summary: List resources
      responses:
        '200':
          description: Success
EOF

curl -sS -X POST "http://127.0.0.1:8080/api/v1/api-definitions/from-openapi?team=team-a" \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  -H "Content-Type: application/yaml" \
  --data-binary @team-a-api.yaml | jq
```

### Step 2: Team B - Dedicated Listener

```bash
# Team B gets dedicated listener on port 8081
cat > team-b-api.yaml <<EOF
openapi: 3.0.0
info:
  title: Team B API
  version: 1.0.0
servers:
  - url: https://team-b.example.com
x-flowplane-filters:
  local_rate_limit:
    stat_prefix: "team_b_ratelimit"
    token_bucket:
      max_tokens: 500
      tokens_per_fill: 500
      fill_interval_ms: 1000
paths:
  /api/v1/data:
    get:
      summary: Get data
      responses:
        '200':
          description: Success
EOF

curl -sS -X POST "http://127.0.0.1:8080/api/v1/api-definitions/from-openapi?team=team-b&listener=team-b-gateway&port=8081" \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  -H "Content-Type: application/yaml" \
  --data-binary @team-b-api.yaml | jq
```

### Step 3: Verify Isolation

```bash
# Team A resources (shared listener, port 10000)
curl -sS -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  http://127.0.0.1:8080/api/v1/routes | jq '.[] | select(.name | contains("team-a"))'

# Team B resources (dedicated listener, port 8081)
curl -sS -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  http://127.0.0.1:8080/api/v1/listeners/team-b-gateway | jq

# Verify Team B has rate limit, Team A doesn't
curl -sS -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  http://127.0.0.1:8080/api/v1/listeners/team-b-gateway | \
  jq '.filterChains[0].filters[0].httpFilters[] | select(.name | contains("ratelimit"))'
```

### Step 4: Test Both Gateways

```bash
# Team A - shared listener
curl -i http://127.0.0.1:10000/api/v1/resources \
  -H "Host: team-a.example.com"

# Team B - dedicated listener
curl -i http://127.0.0.1:8081/api/v1/data \
  -H "Host: team-b.example.com"
```

---

## Tutorial 6: Health Check Configuration (10 minutes)

**Goal**: Configure active health checks for backend cluster

### Step 1: Create Cluster with Health Checks

```bash
curl -sS -X POST http://127.0.0.1:8080/api/v1/clusters \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "backend-with-health",
    "serviceName": "backend",
    "endpoints": [
      {"host": "backend1.internal", "port": 8080},
      {"host": "backend2.internal", "port": 8080},
      {"host": "backend3.internal", "port": 8080}
    ],
    "connectTimeoutSeconds": 5,
    "healthCheck": {
      "path": "/healthz",
      "intervalSeconds": 10,
      "timeoutSeconds": 2,
      "unhealthyThreshold": 3,
      "healthyThreshold": 2,
      "expectedStatuses": [200, 204]
    },
    "circuitBreakers": {
      "maxConnections": 1000,
      "maxPendingRequests": 1000,
      "maxRequests": 1000,
      "maxRetries": 3
    }
  }' | jq
```

### Step 2: Monitor Health Check Status

```bash
# Check Envoy cluster health status
curl -s http://127.0.0.1:9901/clusters | grep backend-with-health

# Look for:
# - health_flags::healthy (endpoint is healthy)
# - health_flags::failed_active_hc (endpoint failed health check)
```

### Step 3: Test Circuit Breaker

```bash
# Generate load to trigger circuit breaker
for i in {1..1100}; do
  curl -s http://127.0.0.1:10000/test > /dev/null &
done

# Check circuit breaker stats
curl -s http://127.0.0.1:9901/stats | grep backend-with-health | grep cx_open
```

---

## Tutorial 7: TLS Termination (15 minutes)

**Goal**: Configure listener with TLS termination

**Prerequisites:** TLS certificates (self-signed acceptable for testing)

### Step 1: Generate Self-Signed Certificate

```bash
# Generate private key
openssl genrsa -out server.key 2048

# Generate certificate
openssl req -new -x509 -key server.key -out server.crt -days 365 \
  -subj "/CN=localhost"

# Convert to PEM
cp server.crt server.pem
cp server.key server-key.pem
```

### Step 2: Create Listener with TLS

```bash
curl -sS -X POST http://127.0.0.1:8080/api/v1/listeners \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "tls-listener",
    "address": "0.0.0.0",
    "port": 10443,
    "protocol": "HTTP",
    "filterChains": [{
      "name": "default",
      "tlsContext": {
        "certificatePath": "/etc/envoy/certs/server.pem",
        "privateKeyPath": "/etc/envoy/certs/server-key.pem"
      },
      "filters": [{
        "name": "envoy.filters.network.http_connection_manager",
        "type": "httpConnectionManager",
        "routeConfigName": "httpbin-routes",
        "httpFilters": [
          {"name": "envoy.filters.http.router", "filter": {"type": "router"}}
        ]
      }]
    }]
  }' | jq
```

### Step 3: Mount Certificates in Envoy

```bash
# If using Docker
docker run -d \
  -p 10443:10443 \
  -v $(pwd)/server.pem:/etc/envoy/certs/server.pem:ro \
  -v $(pwd)/server-key.pem:/etc/envoy/certs/server-key.pem:ro \
  -v $(pwd)/envoy-bootstrap.yaml:/etc/envoy/envoy.yaml \
  envoyproxy/envoy:v1.28-latest
```

### Step 4: Test TLS Connection

```bash
# Test HTTPS connection
curl -i -k https://127.0.0.1:10443/get

# Verify certificate
openssl s_client -connect 127.0.0.1:10443 -showcerts
```

---

## Common Issues & Solutions

### Issue: Resources Created but Envoy Not Responding

**Symptoms:**
```bash
curl http://127.0.0.1:10000/
# Connection refused or 404
```

**Solutions:**
1. Verify Envoy is connected to control plane:
```bash
curl -s http://127.0.0.1:9901/config_dump | jq '.configs[0]'
```

2. Check listener configuration in Envoy:
```bash
curl -s http://127.0.0.1:9901/listeners
```

3. Verify control plane logs:
```bash
docker logs flowplane-cp | grep xDS
```

### Issue: 503 Service Unavailable

**Symptoms:**
```bash
curl http://127.0.0.1:10000/
# HTTP/1.1 503 Service Unavailable
```

**Solutions:**
1. Check cluster health:
```bash
curl -s http://127.0.0.1:9901/clusters | grep your-cluster
```

2. Verify endpoint is reachable:
```bash
curl -v https://your-backend.com
```

3. Check Envoy upstream connections:
```bash
curl -s http://127.0.0.1:9901/stats | grep upstream_cx
```

### Issue: Rate Limit Not Working

**Symptoms:** No 429 responses even after exceeding limits

**Solutions:**
1. Verify filter is configured:
```bash
curl -sS -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  http://127.0.0.1:8080/api/v1/listeners/your-listener | \
  jq '.filterChains[0].filters[0].httpFilters[] | select(.name | contains("ratelimit"))'
```

2. Check rate limit stats in Envoy:
```bash
curl -s http://127.0.0.1:9901/stats | grep local_rate_limit
```

3. Ensure token bucket is not too large:
```json
"token_bucket": {
  "max_tokens": 10,  // Lower for testing
  "tokens_per_fill": 10,
  "fill_interval_ms": 1000
}
```

## Additional Resources

- [Getting Started](getting-started.md) - Basic concepts and setup
- [API Reference](api.md) - Complete endpoint documentation
- [Filters Guide](filters.md) - HTTP filter configuration reference
- [Operations Guide](operations.md) - Production deployment
- [CLI Usage](cli-usage.md) - Command-line workflows
- [Architecture](architecture.md) - System design and components
