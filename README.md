## Flowplane Envoy Control Plane

### The name
Flowplane takes its cue from the Sanskrit word *Pravāha* (प्रवाह), meaning “stream” or “steady flow.” We use the term with respect, as a way to evoke the idea of guiding traffic smoothly through the control plane while honoring its linguistic roots.

The Aim of the CP is initially to provide a resultful interface for Envoy. We will then try to extend this capability to support A2A and MCP protocols

### Overview
Flowplane is an Envoy control plane that keeps listener, route, and cluster configuration in structured Rust/JSON models. Each payload is validated and then translated into Envoy protobufs through `envoy-types`, so you can assemble advanced filter chains—JWT auth, rate limiting, TLS, tracing—without hand-crafting `Any` blobs.

### Before You Start
- Rust toolchain (1.75+ recommended)
- SQLite (for the default embedded database)
- Envoy proxy (when you are ready to point a data-plane instance at the control plane)

### Launch the Control Plane
```bash
FLOWPLANE_XDS_PORT=18003 \
FLOWPLANE_CLUSTER_NAME=my_cluster \
FLOWPLANE_BACKEND_PORT=9090 \
FLOWPLANE_LISTENER_PORT=8080 \
FLOWPLANE_DATABASE_URL=sqlite://./data/flowplane.db \
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
FLOWPLANE_XDS_PORT=18003 \
FLOWPLANE_XDS_TLS_CERT_PATH=certs/xds-server.pem \
FLOWPLANE_XDS_TLS_KEY_PATH=certs/xds-server.key \
FLOWPLANE_XDS_TLS_CLIENT_CA_PATH=certs/xds-ca.pem \
FLOWPLANE_DATABASE_URL=sqlite://./data/flowplane.db \
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

1. Start the control plane. On first launch a bootstrap admin token is emitted once in the logs and
   recorded as `auth.token.seeded` in the audit log.
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
  -X POST "http://127.0.0.1:8080/api/v1/gateways/openapi?name=example" \
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
