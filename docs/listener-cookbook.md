# Listener Cookbook

Listeners bind Envoy to ports and assemble filter chains. This cookbook showcases common HTTP listener configurations using the `/api/v1/listeners` endpoint. Remember: camelCase for control-plane fields (`filterChains`, `httpFilters`), snake_case inside structured filter configs (`stat_prefix`).

## 1. Minimal HTTP Listener
The control plane appends the router filter automatically if you leave it out, but it is shown here for clarity.

```bash
curl -sS -X POST http://127.0.0.1:8080/api/v1/listeners \
  -H 'Authorization: Bearer $FLOWPLANE_TOKEN' \
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
  -H 'Authorization: Bearer $FLOWPLANE_TOKEN' \
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

## 7. Credential Injector for OAuth2
Inject OAuth2 credentials into requests for service-to-service authentication.

```bash
"httpFilters": [
  {
    "name": "envoy.filters.http.credential_injector",
    "filter": {
      "type": "credential_injector",
      "overwrite": false,
      "allow_request_without_credential": false,
      "credential": {
        "name": "oauth2_credential",
        "config": {
          "type_url": "type.googleapis.com/envoy.extensions.http.injected_credentials.oauth2.v3.OAuth2",
          "value": "CgwxMjcuMC4wLjE6ODAQoI4G"
        }
      }
    }
  },
  {"name": "envoy.filters.http.router", "filter": {"type": "router"}}
]
```

The base64-encoded `value` contains the OAuth2 configuration (token endpoint, client credentials, scopes). See [Envoy OAuth2 credential documentation](https://www.envoyproxy.io/docs/envoy/latest/api-v3/extensions/http/injected_credentials/oauth2/v3/oauth2.proto) for the complete configuration structure.

**Use Cases:**
- Zero-trust service mesh authentication
- Backend-for-Frontend (BFF) patterns with token exchange
- Microservices calling external APIs requiring OAuth2

## 8. External Processor Filter
Forward requests to an external gRPC service for custom processing.

```bash
"httpFilters": [
  {
    "name": "envoy.filters.http.ext_proc",
    "filter": {
      "type": "ext_proc",
      "grpc_service": {
        "target_uri": "auth-processor:9000",
        "timeout_seconds": 5
      },
      "failure_mode_allow": false,
      "processing_mode": {
        "request_header_mode": "SEND",
        "response_header_mode": "SKIP",
        "request_body_mode": "NONE",
        "response_body_mode": "NONE"
      },
      "message_timeout_ms": 2000
    }
  },
  {"name": "envoy.filters.http.router", "filter": {"type": "router"}}
]
```

The external processor receives request headers and can:
- Add/modify/remove headers
- Return immediate responses (deny access, return cached data)
- Trigger Envoy to clear route cache

**Use Cases:**
- Custom authorization logic (e.g., checking user permissions in a database)
- Request enrichment (adding tenant context from header claims)
- Dynamic routing decisions
- Content-based rate limiting
- Integration with legacy authentication systems

**Performance Tips:**
- Set `failure_mode_allow: true` for non-critical processing to prevent outages if processor fails
- Use `processing_mode` to minimize data sent to processor (e.g., `SKIP` for response headers if not needed)
- Keep `message_timeout_ms` low (1-5 seconds) to avoid blocking traffic
- Consider using `request_body_mode: "NONE"` unless you need body inspection

## Filter Chain Composition

### Filter Ordering Principles

HTTP filters execute in the order they appear in the `httpFilters` array, with the **router filter always last**. Proper ordering is critical for correct behavior:

**General Ordering Guidelines:**
1. **Rate Limiting First** - Reject excess traffic before expensive operations (auth, transformation)
2. **Authentication Second** - Verify identity before authorization or processing
3. **Authorization Third** - Check permissions after identity is established
4. **Transformation Fourth** - Modify requests/responses after auth checks pass
5. **Router Last** - Always the terminal filter that forwards to upstream

**Example Order:**
```
local_rate_limit → jwt_authn → ext_proc (authz) → header_mutation → router
```

**Why This Matters:**
- Rate limiting before JWT avoids wasting CPU on invalid tokens during DDoS
- JWT before ext_proc allows external processor to trust claims in metadata
- Header mutation after authz prevents bypassing security by manipulating headers
- Router last ensures all filters have processed the request

## 9. Multi-Filter Composition: Rate Limiting + JWT Auth

Combine global rate limiting with JWT authentication for a secure API gateway.

```bash
curl -sS -X POST http://127.0.0.1:8080/api/v1/listeners \
  -H 'Authorization: Bearer $FLOWPLANE_TOKEN' \
  -H 'Content-Type: application/json' \
  -d '{
    "name": "secure-api-listener",
    "address": "0.0.0.0",
    "port": 8443,
    "protocol": "HTTP",
    "filterChains": [
      {
        "name": "default",
        "filters": [
          {
            "name": "envoy.filters.network.http_connection_manager",
            "type": "httpConnectionManager",
            "routeConfigName": "api-routes",
            "httpFilters": [
              {
                "name": "envoy.filters.http.local_ratelimit",
                "filter": {
                  "type": "local_rate_limit",
                  "stat_prefix": "api_global",
                  "token_bucket": {
                    "max_tokens": 1000,
                    "tokens_per_fill": 1000,
                    "fill_interval_ms": 1000
                  },
                  "status_code": 429
                }
              },
              {
                "name": "envoy.filters.http.jwt_authn",
                "filter": {
                  "type": "jwt_authn",
                  "providers": {
                    "auth0": {
                      "issuer": "https://your-tenant.auth0.com/",
                      "audiences": ["https://api.example.com"],
                      "jwks": {
                        "remote": {
                          "httpUri": {
                            "uri": "https://your-tenant.auth0.com/.well-known/jwks.json",
                            "cluster": "auth0-jwks",
                            "timeoutMs": 2000
                          }
                        }
                      },
                      "payloadInMetadata": "jwt_payload"
                    }
                  },
                  "rules": [
                    {
                      "match": {"path": {"type": "prefix", "value": "/"}},
                      "requires": {
                        "type": "provider_name",
                        "providerName": "auth0"
                      }
                    }
                  ]
                }
              },
              {"name": "envoy.filters.http.router", "filter": {"type": "router"}}
            ]
          }
        ]
      }
    ]
  }'
```

**Request Flow:**
1. **Rate limit check** (1000 req/s max) → 429 if exceeded
2. **JWT validation** → 401 if missing/invalid token
3. **Route to upstream** → Request forwarded with JWT claims in metadata

**Per-Route Overrides:**
Use `typedPerFilterConfig` in routes to adjust limits for specific endpoints:

```json
"typedPerFilterConfig": {
  "envoy.filters.http.local_ratelimit": {
    "stat_prefix": "premium_endpoint",
    "token_bucket": {
      "max_tokens": 5000,
      "tokens_per_fill": 5000,
      "fill_interval_ms": 1000
    }
  },
  "envoy.filters.http.jwt_authn": {
    "requirement_name": "allow_missing"
  }
}
```

## 10. Advanced Composition: Custom Authorization Pipeline

Build a complete authorization pipeline with external processing and credential injection.

```bash
"httpFilters": [
  {
    "name": "envoy.filters.http.local_ratelimit",
    "filter": {
      "type": "local_rate_limit",
      "stat_prefix": "authz_pipeline",
      "token_bucket": {
        "max_tokens": 500,
        "tokens_per_fill": 500,
        "fill_interval_ms": 1000
      }
    }
  },
  {
    "name": "envoy.filters.http.jwt_authn",
    "filter": {
      "type": "jwt_authn",
      "providers": {
        "keycloak": {
          "issuer": "https://keycloak.example.com/realms/production",
          "audiences": ["backend-api"],
          "jwks": {
            "remote": {
              "httpUri": {
                "uri": "https://keycloak.example.com/realms/production/protocol/openid-connect/certs",
                "cluster": "keycloak-jwks",
                "timeoutMs": 3000
              }
            }
          },
          "payloadInMetadata": "jwt_claims"
        }
      },
      "rules": [
        {
          "match": {"path": {"type": "prefix", "value": "/"}},
          "requires": {
            "type": "provider_name",
            "providerName": "keycloak"
          }
        }
      ]
    }
  },
  {
    "name": "envoy.filters.http.ext_proc",
    "filter": {
      "type": "ext_proc",
      "grpc_service": {
        "target_uri": "authz-service:9000",
        "timeout_seconds": 3
      },
      "failure_mode_allow": false,
      "processing_mode": {
        "request_header_mode": "SEND",
        "response_header_mode": "SKIP",
        "request_body_mode": "NONE",
        "response_body_mode": "NONE"
      },
      "message_timeout_ms": 1500,
      "request_attributes": ["jwt_claims"]
    }
  },
  {
    "name": "envoy.filters.http.credential_injector",
    "filter": {
      "type": "credential_injector",
      "overwrite": false,
      "allow_request_without_credential": false,
      "credential": {
        "name": "service_account_token",
        "config": {
          "type_url": "type.googleapis.com/envoy.extensions.http.injected_credentials.oauth2.v3.OAuth2",
          "value": "CgwxMjcuMC4wLjE6ODAQoI4G"
        }
      }
    }
  },
  {
    "name": "envoy.filters.http.header_mutation",
    "filter": {
      "type": "header_mutation",
      "request_headers_to_add": [
        {
          "header": {
            "key": "X-Request-ID",
            "value": "%REQ(x-request-id)%"
          }
        },
        {
          "header": {
            "key": "X-Forwarded-For",
            "value": "%DOWNSTREAM_REMOTE_ADDRESS%"
          }
        }
      ],
      "request_headers_to_remove": ["X-Internal-Debug"]
    }
  },
  {"name": "envoy.filters.http.router", "filter": {"type": "router"}}
]
```

**Request Flow:**
1. **Rate limit** (500 req/s) → Prevents DDoS
2. **JWT validation** (Keycloak) → Extracts user identity
3. **External authorization** → Custom authz service checks permissions using JWT claims
4. **Credential injection** → Adds service account token for upstream APIs
5. **Header mutation** → Adds correlation headers, removes debug headers
6. **Router** → Forwards enriched request to upstream

**Why This Order Works:**
- Rate limiting first prevents wasteful processing during attacks
- JWT validation establishes identity before authorization
- External processor can access JWT claims from metadata
- Credential injection happens after authorization passes
- Header mutation is last transformation before routing
- Router always terminates the filter chain

## 11. Common Filter Chain Patterns

### Pattern 1: Public API with Rate Limiting
```bash
local_rate_limit → cors → router
```
Use for public APIs without authentication. CORS after rate limit to avoid preflight abuse.

### Pattern 2: Authenticated API
```bash
local_rate_limit → jwt_authn → router
```
Standard pattern for protected APIs with token-based authentication.

### Pattern 3: Microservices Gateway
```bash
local_rate_limit → jwt_authn → credential_injector → header_mutation → router
```
Validates user JWT, injects service account credentials, adds tracing headers.

### Pattern 4: Custom Authorization
```bash
local_rate_limit → jwt_authn → ext_proc → router
```
External processor performs complex authorization logic (ABAC, RBAC from database).

### Pattern 5: Enterprise Security
```bash
rate_limit (distributed) → jwt_authn → ext_proc → health_check → router
```
Distributed rate limiting, JWT auth, custom authz, health check passthrough.

### Pattern 6: Content Transformation
```bash
local_rate_limit → jwt_authn → ext_proc → custom_response → router
```
External processor transforms request/response bodies, custom error responses.

## Operations
- List listeners: `GET /api/v1/listeners`
- Fetch one: `GET /api/v1/listeners/{name}`
- Update: `PUT /api/v1/listeners/{name}`
- Delete: `DELETE /api/v1/listeners/{name}`

Combine these recipes with the [cluster](cluster-cookbook.md) and [routing](routing-cookbook.md) cookbooks to assemble end-to-end gateways quickly.
