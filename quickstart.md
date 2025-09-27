# Flowplane Quickstart

This quickstart walks through bootstrapping the control plane, creating a personal access token, and
calling a secured API endpoint. It assumes you are running everything locally on macOS or Linux.

## 1. Prerequisites

- Rust toolchain (1.75 or newer)
- SQLite (bundled with macOS and most Linux distributions)
- An empty working directory for Flowplane databases (default: `./data`)

## 2. Apply Migrations

The repo ships with SQLx migrations. Run them once before starting the control plane:

```bash
cargo run --bin run_migrations -- \
  --database-url sqlite://./data/flowplane.db
```

## 3. Launch the Control Plane

```bash
FLOWPLANE_DATABASE_URL=sqlite://./data/flowplane.db \
FLOWPLANE_API_BIND_ADDRESS=127.0.0.1 \
FLOWPLANE_API_PORT=8080 \
FLOWPLANE_XDS_PORT=18000 \
cargo run --bin flowplane
```

On first startup a bootstrap admin token is created automatically. Capture the log output once:

```text
WARN flowplane::openapi::defaults: Seeded bootstrap admin personal access token; store it securely
```

Copy the `fp_pat_...` value into a secret store. The control plane never shows it again.

### Optional: Serve the Admin API over HTTPS

Provide the following variables at startup if you already have certificates:

```bash
FLOWPLANE_API_TLS_ENABLED=true \
FLOWPLANE_API_TLS_CERT_PATH=/path/to/cert.pem \
FLOWPLANE_API_TLS_KEY_PATH=/path/to/key.pem \
FLOWPLANE_API_TLS_CHAIN_PATH=/path/to/chain.pem \
cargo run --bin flowplane
```

Flowplane validates the PEM files during startup and fails fast on unreadable files, mismatched key pairs, or expired certificates. After rotating certificates, restart the control plane to pick up the new material. See [`docs/tls.md`](docs/tls.md) for certificate sourcing guidance.

## 4. Issue a Scoped Token

Use the bootstrap credential to mint a token with tighter scopes:

```bash
export FLOWPLANE_BOOTSTRAP_TOKEN="fp_pat_..."

curl -sS \
  -X POST http://127.0.0.1:8080/api/v1/tokens \
  -H "Authorization: Bearer $FLOWPLANE_BOOTSTRAP_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
        "name": "dev-console",
        "scopes": ["clusters:read", "routes:read", "listeners:read"],
        "description": "Token used by the local developer console"
      }'
```

Store the returned token securely and revoke the bootstrap credential once you have at least one
replacement.

## 5. Call the API

```bash
export FLOWPLANE_TOKEN="fp_pat_..."  # newly created token

curl -sS \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  http://127.0.0.1:8080/api/v1/clusters
```

Expect a `200 OK` response containing the default gateway cluster seeded at startup.

## 6. Monitor Metrics & Audit

- Prometheus metrics are exposed at `http://127.0.0.1:9090/metrics` (adjust the port via
  `FLOWPLANE_METRICS_PORT`). Authentication counters are prefixed with `auth_`.
- Audit events (e.g., `auth.token.created`, `auth.token.authenticated`) are written to the `audit_log`
  table in the primary database. Tail them with `sqlite3 data/flowplane.db 'SELECT * FROM audit_log'`.

## 7. Rotate or Revoke Tokens

Rotate the secret when you suspect exposure:

```bash
curl -sS -X POST \
  -H "Authorization: Bearer $FLOWPLANE_BOOTSTRAP_TOKEN" \
  http://127.0.0.1:8080/api/v1/tokens/<token-id>/rotate
```

Revoke a token to disable it permanently:

```bash
curl -sS -X DELETE \
  -H "Authorization: Bearer $FLOWPLANE_BOOTSTRAP_TOKEN" \
  http://127.0.0.1:8080/api/v1/tokens/<token-id>
```

For CLI alternatives see [`docs/token-management.md`](docs/token-management.md).

## Next Steps

- Review [`docs/authentication.md`](docs/authentication.md) for scope details and observability hooks.
- Import your first OpenAPI spec with `POST /api/v1/gateways/openapi`.
- Explore the cookbooks in `docs/` for advanced listener, route, and cluster patterns.
