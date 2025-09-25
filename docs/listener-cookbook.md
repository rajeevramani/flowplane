# Listener Cookbook

Listeners bind Envoy to ports and assemble filter chains. This cookbook showcases common HTTP listener configurations using the `/api/v1/listeners` endpoint. Remember: camelCase for control-plane fields (`filterChains`, `httpFilters`), snake_case inside structured filter configs (`stat_prefix`).

## 1. Minimal HTTP Listener
The control plane appends the router filter automatically if you leave it out, but it is shown here for clarity.

```bash
curl -sS -X POST http://127.0.0.1:8080/api/v1/listeners \
  -H 'Content-Type: application/json' \
  -d '{
    "name": "edge-listener",
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
            "routeConfigName": "edge-routes",
            "httpFilters": [
              {"name": "envoy.filters.http.router", "filter": {"type": "router"}}
            ]
          }
        ]
      }
    ]
  }'
```

## 2. Listener-Wide Local Rate Limit
Throttle globally across the listener before requests hit per-route overrides.

```bash
"httpFilters": [
  {
    "name": "envoy.filters.http.local_ratelimit",
    "filter": {
      "type": "local_rate_limit",
      "stat_prefix": "listener_global",
      "token_bucket": {
        "max_tokens": 200,
        "tokens_per_fill": 200,
        "fill_interval_ms": 1000
      },
      "status_code": 429
    }
  },
  {"name": "envoy.filters.http.router", "filter": {"type": "router"}}
]
```

Requests that exceed the global bucket receive HTTP 429 before hitting upstreams. Combine with per-route limits to protect specific endpoints.

## 3. JWT Authentication Filter
Attach JWT auth ahead of the router. See [filters.md](filters.md#jwt-authentication) for full provider configuration.

```bash
"httpFilters": [
  {
    "name": "envoy.filters.http.jwt_authn",
    "filter": {
      "type": "jwt_authn",
      "providers": {
        "primary": {
          "issuer": "https://issuer.example.com",
          "audiences": ["frontend-app"],
          "jwks": {
            "remote": {
              "httpUri": {
                "uri": "https://issuer.example.com/.well-known/jwks.json",
                "cluster": "jwks-cluster",
                "timeoutMs": 1500
              }
            }
          },
          "payloadInMetadata": "jwt_payload"
        }
      },
      "rules": [
        {
          "match": {"path": {"type": "prefix", "value": "/api"}},
          "requires": {
            "type": "provider_name",
            "providerName": "primary"
          }
        }
      ]
    }
  },
  {"name": "envoy.filters.http.router", "filter": {"type": "router"}}
]
```

Add route-level overrides with `typedPerFilterConfig` when certain endpoints need relaxed or stricter requirements.

## 4. Access Logging & Tracing
Emit access logs and configure tracing.

```bash
{
  "accessLog": {
    "path": "/var/log/envoy/access.log",
    "format": "[%START_TIME%] %REQ(:METHOD)% %REQ(X-REQUEST-ID)% %UPSTREAM_HOST%\n"
  },
  "tracing": {
    "provider": "envoy.tracers.opencensus",
    "config": {
      "driver": "logging"
    }
  }
}
```

Place this block inside the `httpConnectionManager` filter body along with `routeConfigName` and `httpFilters`.

## 5. TLS-Terminating Listener
Wrap the filter chain with a downstream TLS context to terminate HTTPS connections at Envoy.

```bash
{
  "filterChains": [
    {
      "name": "tls-chain",
      "tlsContext": {
        "certChainFile": "/etc/envoy/certs/server.crt",
        "privateKeyFile": "/etc/envoy/certs/server.key",
        "caCertFile": "/etc/envoy/certs/ca.crt",
        "requireClientCertificate": true
      },
      "filters": [
        {
          "name": "envoy.filters.network.http_connection_manager",
          "type": "httpConnectionManager",
          "routeConfigName": "secure-routes",
          "httpFilters": [
            {"name": "envoy.filters.http.router", "filter": {"type": "router"}}
          ]
        }
      ]
    }
  ]
}
```

Use this pattern for mTLS or when you want Envoy to front HTTPS traffic and pass HTTP to upstream services.

## 6. TCP Proxy Listener
Expose raw TCP services (databases, custom protocols) through Envoy.

```bash
curl -sS -X POST http://127.0.0.1:8080/api/v1/listeners \
  -H 'Content-Type: application/json' \
  -d '{
    "name": "redis-listener",
    "address": "0.0.0.0",
    "port": 6379,
    "protocol": "TCP",
    "filterChains": [
      {
        "filters": [
          {
            "name": "envoy.filters.network.tcp_proxy",
            "type": "tcpProxy",
            "cluster": "redis-cluster"
          }
        ]
      }
    ]
  }'
```

## Operations
- List listeners: `GET /api/v1/listeners`
- Fetch one: `GET /api/v1/listeners/{name}`
- Update: `PUT /api/v1/listeners/{name}`
- Delete: `DELETE /api/v1/listeners/{name}`

Combine these recipes with the [cluster](cluster-cookbook.md) and [routing](routing-cookbook.md) cookbooks to assemble end-to-end gateways quickly.
