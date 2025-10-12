## Flowplane Envoy Control Plane

### The name
Flowplane takes its cue from the Sanskrit word *Pravāha* (प्रवाह), meaning “stream” or “steady flow.” We use the term with respect, as a way to evoke the idea of guiding traffic smoothly through the control plane while honoring its linguistic roots.

The Aim of the CP is initially to provide a resultful interface for Envoy. We will then try to extend this capability to support A2A and MCP protocols

### Overview
Flowplane is an Envoy control plane that keeps listener, route, and cluster configuration in structured Rust/JSON models. Each payload is validated and then translated into Envoy protobufs through `envoy-types`, so you can assemble advanced filter chains—JWT auth, rate limiting, TLS, tracing—without hand-crafting `Any` blobs.

### Before You Start
- Rust toolchain (1.75+ required)
- SQLite (for the default embedded database)
- Envoy proxy (when you are ready to point a data-plane instance at the control plane)
- **Bootstrap Token**: Generate a secure token for initial admin access (see Authentication section below)

**Quick Start with Docker:** For the fastest way to get started, see [README-DOCKER.md](README-DOCKER.md) for Docker Compose instructions.

### Launch the Control Plane
```bash
# Generate a secure bootstrap token first
export BOOTSTRAP_TOKEN=$(openssl rand -base64 32)

# Minimal production start
DATABASE_URL=sqlite://./data/flowplane.db \
BOOTSTRAP_TOKEN="$BOOTSTRAP_TOKEN" \
FLOWPLANE_API_BIND_ADDRESS=0.0.0.0 \
cargo run --bin flowplane

# With custom ports (optional)
DATABASE_URL=sqlite://./data/flowplane.db \
BOOTSTRAP_TOKEN="$BOOTSTRAP_TOKEN" \
FLOWPLANE_API_BIND_ADDRESS=0.0.0.0 \
FLOWPLANE_API_PORT=8080 \
FLOWPLANE_XDS_PORT=50051 \
cargo run --bin flowplane
```

The REST API is available on `http://127.0.0.1:8080`. Open the interactive API reference at **`http://127.0.0.1:8080/swagger-ui`** (OpenAPI JSON is served at `/api-docs/openapi.json`).

### Secure the xDS Channel
Protect Envoy → control plane traffic with TLS or mutual TLS by exporting the following environment variables before starting Flowplane:

- `FLOWPLANE_XDS_TLS_CERT_PATH` – PEM-encoded server certificate chain returned to Envoy.
- `FLOWPLANE_XDS_TLS_KEY_PATH` – PEM-encoded private key matching the certificate chain.
- `FLOWPLANE_XDS_TLS_CLIENT_CA_PATH` – (optional) CA bundle used to validate Envoy client certificates.
- `FLOWPLANE_XDS_TLS_REQUIRE_CLIENT_CERT` – (optional) defaults to `true`; set to `false` to allow TLS without client authentication.

Example launch with mutual TLS:

```bash
DATABASE_URL=sqlite://./data/flowplane.db \
BOOTSTRAP_TOKEN="$(openssl rand -base64 32)" \
FLOWPLANE_API_BIND_ADDRESS=0.0.0.0 \
FLOWPLANE_XDS_PORT=50051 \
FLOWPLANE_XDS_TLS_CERT_PATH=certs/xds-server.pem \
FLOWPLANE_XDS_TLS_KEY_PATH=certs/xds-server.key \
FLOWPLANE_XDS_TLS_CLIENT_CA_PATH=certs/xds-ca.pem \
cargo run --bin flowplane
```

Point Envoy at the xDS server using a TLS-enabled cluster and reference the same CA, client certificate, and key inside the Envoy bootstrap (see `envoy-test.yaml` for a full example).

- Flowplane seeds a shared gateway trio (`default-gateway-cluster`, `default-gateway-routes`, `default-gateway-listener`) during startup. They fuel the default OpenAPI import path and are protected from deletion so the shared listener keeps working for every team.

### Enable HTTPS for the Admin API
Enable TLS termination on the admin API by supplying certificate paths at startup:

- `FLOWPLANE_API_TLS_ENABLED` – set to `true`, `1`, `yes`, or `on` to enable HTTPS (defaults to HTTP when unset).
- `FLOWPLANE_API_TLS_CERT_PATH` – PEM-encoded leaf certificate served to clients.
- `FLOWPLANE_API_TLS_KEY_PATH` – PEM-encoded private key matching the certificate.
- `FLOWPLANE_API_TLS_CHAIN_PATH` *(optional)* – PEM bundle with intermediate issuers if clients need the full chain.

When these variables are present the server binds HTTPS, logs the certificate subject and expiry, and rejects startup if the files are missing, unreadable, expired, or mismatched. See [`docs/tls.md`](docs/tls.md) for workflows covering ACME automation, corporate PKI, and local development certificates.

### Authenticate API Calls
Flowplane now protects every REST endpoint with bearer authentication:

1. Start the control plane. On first launch, a bootstrap admin token is **displayed in a prominent banner**
   with security warnings and also recorded as `auth.token.seeded` in the audit log.

   ```bash
   # Extract the token from Docker logs
   docker-compose logs control-plane 2>&1 | grep -oP 'token: \Kfp_pat_[^\s]+'

   # Or from local logs
   cargo run --bin flowplane 2>&1 | grep "Token:"
   ```
2. Store the value securely (e.g., in a secrets manager) and use it to create scoped tokens via the
   API or CLI:

   ```bash
   export FLOWPLANE_ADMIN_TOKEN="fp_pat_..."

   curl -sS \
     -X POST http://127.0.0.1:8080/api/v1/tokens \
     -H "Authorization: Bearer $FLOWPLANE_ADMIN_TOKEN" \
     -H "Content-Type: application/json" \
     -d '{
           "name": "ci-pipeline",
           "scopes": ["clusters:write", "routes:write", "listeners:read"]
         }'
   ```

3. Use the returned token for automation and discard the bootstrap credential.

Scopes map one-to-one with API groups (`clusters:*`, `routes:*`, `listeners:*`, `tokens:*`,
`gateways:import`). See [`docs/authentication.md`](docs/authentication.md) for details and
[`docs/token-management.md`](docs/token-management.md) for CLI recipes.

### Build Your First Gateway
Follow the [step-by-step guide](docs/getting-started.md) to:

1. Register a cluster with upstream endpoints
2. Publish a route configuration with optional per-route rate limiting
3. Create a listener that wires in global filters (JWT auth, Local Rate Limit, tracing)
4. Verify requests flowing through Envoy

Each step includes `curl` examples and the JSON payloads the API expects.

### Bootstrap From OpenAPI
If you already have an OpenAPI 3.0 spec, Flowplane can generate clusters, routes, and a listener in one call:

```bash
curl -sS \
  -X POST "http://127.0.0.1:8080/api/v1/api-definitions/from-openapi?team=example" \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  -H 'Content-Type: application/json' \
  --data-binary @openapi.json
```

The endpoint accepts either JSON or YAML documents. Flowplane derives upstream clusters from the spec’s `servers` section, builds route matches from `paths`, and publishes a listener on the port you choose (override with `address` / `port` query parameters).

By default the generated routes join the shared gateway listener `default-gateway-listener` on port `10000`, so multiple specs can coexist without wrestling over listener names or ports. To provision a dedicated listener instead, supply query parameters such as `listener=<custom-name>` (optionally `port`, `bind_address`, and `protocol`) and Flowplane will create separate route and listener resources for that gateway.

### Rate Limiting at a Glance
Flowplane models Envoy’s Local Rate Limit filter both globally and per-route:

- **Listener-wide** limits: add a `local_rate_limit` entry to the HTTP filter chain when you create/update a listener. All requests passing through that connection manager share the token bucket.
- **Route-specific** limits: attach a Local Rate Limit scoped config via `typedPerFilterConfig` on routes, virtual hosts, or weighted clusters to tailor traffic policies.

See [docs/filters.md](docs/filters.md#local-rate-limit) for detailed examples of both patterns, including how to combine Local Rate Limit with JWT authentication.

### Environment Variables Reference

#### Core Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `DATABASE_URL` | *required* | SQLite or PostgreSQL connection string (e.g., `sqlite://./data/flowplane.db`) |
| `BOOTSTRAP_TOKEN` | *required* | Secure token (min 32 chars) for creating initial admin PAT. Generate with `openssl rand -base64 32` |
| `FLOWPLANE_API_BIND_ADDRESS` | `127.0.0.1` | API server bind address (use `0.0.0.0` for Docker/remote access) |
| `FLOWPLANE_API_PORT` | `8080` | HTTP API server port |
| `FLOWPLANE_XDS_BIND_ADDRESS` | `0.0.0.0` | xDS gRPC server bind address |
| `FLOWPLANE_XDS_PORT` | `50051` | xDS gRPC server port |

#### TLS Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `FLOWPLANE_API_TLS_ENABLED` | `false` | Enable HTTPS for API (set to `true`, `1`, `yes`, or `on`) |
| `FLOWPLANE_API_TLS_CERT_PATH` | - | PEM-encoded API server certificate |
| `FLOWPLANE_API_TLS_KEY_PATH` | - | PEM-encoded API server private key |
| `FLOWPLANE_API_TLS_CHAIN_PATH` | - | Optional intermediate certificate chain |
| `FLOWPLANE_XDS_TLS_CERT_PATH` | - | PEM-encoded xDS server certificate |
| `FLOWPLANE_XDS_TLS_KEY_PATH` | - | PEM-encoded xDS server private key |
| `FLOWPLANE_XDS_TLS_CLIENT_CA_PATH` | - | CA bundle for validating Envoy client certificates |
| `FLOWPLANE_XDS_TLS_REQUIRE_CLIENT_CERT` | `true` | Require client certificate for mTLS |

#### Observability

| Variable | Default | Description |
|----------|---------|-------------|
| `RUST_LOG` | `info` | Logging level (`error`, `warn`, `info`, `debug`, `trace`) |
| `FLOWPLANE_LOG_LEVEL` | `info` | Alternative logging level configuration |
| `FLOWPLANE_ENABLE_METRICS` | `true` | Enable Prometheus metrics export |
| `FLOWPLANE_ENABLE_TRACING` | `false` | Enable OpenTelemetry tracing |
| `FLOWPLANE_SERVICE_NAME` | `flowplane` | Service name for tracing |
| `FLOWPLANE_JAEGER_ENDPOINT` | - | Jaeger collector endpoint for traces |

#### CLI Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `FLOWPLANE_TOKEN` | - | Personal access token for CLI authentication |
| `FLOWPLANE_BASE_URL` | `http://127.0.0.1:8080` | Control plane API base URL for CLI |

#### Legacy Development Variables

These variables are for simple development mode only (not needed for production):

| Variable | Description |
|----------|-------------|
| `FLOWPLANE_CLUSTER_NAME` | Demo cluster name (dev only) |
| `FLOWPLANE_ROUTE_NAME` | Demo route name (dev only) |
| `FLOWPLANE_LISTENER_NAME` | Demo listener name (dev only) |
| `FLOWPLANE_BACKEND_ADDRESS` | Demo backend address (dev only) |
| `FLOWPLANE_BACKEND_PORT` | Demo backend port (dev only) |
| `FLOWPLANE_LISTENER_PORT` | Demo listener port (dev only) |

### API Endpoints

Flowplane exposes a comprehensive REST API for managing all control plane resources. All endpoints require bearer authentication except for Swagger UI.

#### Interactive Documentation
- **Swagger UI:** `http://127.0.0.1:8080/swagger-ui/`
- **OpenAPI JSON:** `http://127.0.0.1:8080/api-docs/openapi.json`

#### Authentication & Tokens

| Method | Endpoint | Description | Required Scope |
|--------|----------|-------------|----------------|
| POST | `/api/v1/tokens` | Create new token | `tokens:write` |
| GET | `/api/v1/tokens` | List all tokens | `tokens:read` |
| GET | `/api/v1/tokens/{id}` | Get token details | `tokens:read` |
| PATCH | `/api/v1/tokens/{id}` | Update token scopes/name | `tokens:write` |
| DELETE | `/api/v1/tokens/{id}` | Revoke token | `tokens:write` |
| POST | `/api/v1/tokens/{id}/rotate` | Rotate token secret | `tokens:write` |

#### Clusters

| Method | Endpoint | Description | Required Scope |
|--------|----------|-------------|----------------|
| POST | `/api/v1/clusters` | Create cluster | `clusters:write` |
| GET | `/api/v1/clusters` | List clusters | `clusters:read` |
| GET | `/api/v1/clusters/{name}` | Get cluster | `clusters:read` |
| PUT | `/api/v1/clusters/{name}` | Update cluster | `clusters:write` |
| DELETE | `/api/v1/clusters/{name}` | Delete cluster | `clusters:write` |

#### Routes

| Method | Endpoint | Description | Required Scope |
|--------|----------|-------------|----------------|
| POST | `/api/v1/routes` | Create route | `routes:write` |
| GET | `/api/v1/routes` | List routes | `routes:read` |
| GET | `/api/v1/routes/{name}` | Get route | `routes:read` |
| PUT | `/api/v1/routes/{name}` | Update route | `routes:write` |
| DELETE | `/api/v1/routes/{name}` | Delete route | `routes:write` |

#### Listeners

| Method | Endpoint | Description | Required Scope |
|--------|----------|-------------|----------------|
| POST | `/api/v1/listeners` | Create listener | `listeners:write` |
| GET | `/api/v1/listeners` | List listeners | `listeners:read` |
| GET | `/api/v1/listeners/{name}` | Get listener | `listeners:read` |
| PUT | `/api/v1/listeners/{name}` | Update listener | `listeners:write` |
| DELETE | `/api/v1/listeners/{name}` | Delete listener | `listeners:write` |

#### API Definitions (BFF/Platform API)

| Method | Endpoint | Description | Required Scope |
|--------|----------|-------------|----------------|
| POST | `/api/v1/api-definitions` | Create BFF API definition | `api-definitions:write` |
| GET | `/api/v1/api-definitions` | List API definitions | `api-definitions:read` |
| GET | `/api/v1/api-definitions/{id}` | Get API definition | `api-definitions:read` |
| PATCH | `/api/v1/api-definitions/{id}` | Update API definition | `api-definitions:write` |
| POST | `/api/v1/api-definitions/from-openapi` | Import OpenAPI spec | `api-definitions:write` |
| POST | `/api/v1/api-definitions/{id}/routes` | Append route to API | `api-definitions:write` |
| GET | `/api/v1/api-definitions/{id}/bootstrap` | Get Envoy bootstrap config | `api-definitions:read` |

#### Reports & Analytics

| Method | Endpoint | Description | Required Scope |
|--------|----------|-------------|----------------|
| GET | `/api/v1/reports/route-flows` | Get route flow analysis (listener → route → cluster → endpoints) | `reports:read` |

Supports pagination via `limit` and `offset` query parameters. Team-scoped tokens see only their team's routes.

#### Token Scopes

Scopes control access to API groups:

- `tokens:read`, `tokens:write` - Token management
- `clusters:read`, `clusters:write` - Cluster resources
- `routes:read`, `routes:write` - Route configuration resources
- `listeners:read`, `listeners:write` - Listener resources
- `api-definitions:read`, `api-definitions:write` - API definition (Platform API) resources
- `reports:read` - Access to reporting and analytics endpoints
- `admin:all` - Super admin scope granting full access across all resources and teams
- `team:<team-name>:<resource>:<action>` - Team-scoped access pattern (e.g., `team:platform:routes:read`)

### Documentation Map
- [`docs/getting-started.md`](docs/getting-started.md) – From zero to envoy traffic: API walkthrough with clusters, routes, listeners, and verification steps.
- [`docs/cluster-cookbook.md`](docs/cluster-cookbook.md) – Common cluster patterns (TLS, health checks, circuit breakers, DNS).
- [`docs/routing-cookbook.md`](docs/routing-cookbook.md) – Route action recipes (forward, weighted, redirects), matcher combinations, and scoped filters.
- [`docs/listener-cookbook.md`](docs/listener-cookbook.md) – Listener setups covering global filters, JWT auth, TLS termination, and TCP proxying.
- [`docs/gateway-recipes.md`](docs/gateway-recipes.md) – End-to-end API gateway scenarios combining clusters, routes, and listeners.
- [`docs/filters.md`](docs/filters.md) – HTTP filter registry, Local Rate Limit usage, JWT auth providers and scoped overrides, plus extension guidelines.
- [`docs/config-model.md`](docs/config-model.md) – Listener, route, and cluster schema reference and how scoped configs attach to Envoy resources.
- [`docs/testing.md`](docs/testing.md) – Test suite commands, smoke scripts, and manual validation tips.
- [`docs/architecture.md`](docs/architecture.md) – Module layout and design principles.
- [`docs/contributing.md`](docs/contributing.md) – Coding standards and PR expectations.

### Staying Productive
- `GET /api/v1/clusters`, `GET /api/v1/routes`, `GET /api/v1/listeners` show what is currently stored.
- `scripts/smoke-listener.sh` provisions a demo stack against `httpbin.org`; use it as a reference or a sanity check after changes.
- Bruno workspace under `bruno/` bundles HTTP requests (create cluster/route/listener, add rate limits, enable JWT) for quick testing. Import the folder directly into the Bruno app.

### Contributing & Roadmap
We welcome issues and pull requests. Run `cargo fmt`, `cargo clippy -- -D warnings`, and `cargo test` before submitting changes. See [`docs/contributing.md`](docs/contributing.md) for more details.

Upcoming areas of exploration include extending the HTTP filter catalog, MCP protocol support, and richer observability hooks. Contributions that keep the configuration surface consistent and testable are especially appreciated.
