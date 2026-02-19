# Configuration Reference

This document provides a complete reference for all Flowplane configuration options.

## Environment Variables

All environment variables use the `FLOWPLANE_` prefix.

### Core Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `FLOWPLANE_DATABASE_URL` | `postgresql://flowplane:flowplane@localhost:5432/flowplane` | Database connection URL |
| `FLOWPLANE_API_PORT` | `8080` | REST API port |
| `FLOWPLANE_API_BIND_ADDRESS` | `127.0.0.1` | REST API bind address |
| `FLOWPLANE_XDS_PORT` | `18000` | xDS gRPC port |
| `FLOWPLANE_XDS_BIND_ADDRESS` | `0.0.0.0` | xDS bind address |
| `FLOWPLANE_UI_ORIGIN` | `http://localhost:3000` | CORS allowed origin for UI |
| `FLOWPLANE_COOKIE_SECURE` | `true` | Require HTTPS for session cookies (set to `false` for local development) |

### Database

| Variable | Default | Description |
|----------|---------|-------------|
| `FLOWPLANE_DATABASE_MAX_CONNECTIONS` | `10` | Max connection pool size |
| `FLOWPLANE_DATABASE_MIN_CONNECTIONS` | `0` | Min connection pool size |
| `FLOWPLANE_DATABASE_CONNECT_TIMEOUT_SECONDS` | `10` | Connection timeout |
| `FLOWPLANE_DATABASE_IDLE_TIMEOUT_SECONDS` | `600` | Idle connection timeout |
| `FLOWPLANE_DATABASE_AUTO_MIGRATE` | `true` | Run migrations on startup |

### Observability

| Variable | Default | Description |
|----------|---------|-------------|
| `FLOWPLANE_ENABLE_METRICS` | `true` | Enable Prometheus metrics |
| `FLOWPLANE_METRICS_PORT` | `9090` | Metrics endpoint port |
| `FLOWPLANE_ENABLE_TRACING` | `true` | Enable distributed tracing |
| `FLOWPLANE_OTLP_ENDPOINT` | `http://localhost:4317` | OpenTelemetry collector endpoint |
| `FLOWPLANE_TRACE_SAMPLING_RATIO` | `1.0` | Trace sampling ratio (0.0-1.0) |
| `FLOWPLANE_SERVICE_NAME` | `flowplane` | Service name for traces |
| `FLOWPLANE_LOG_LEVEL` | `info` | Log level (trace, debug, info, warn, error) |
| `FLOWPLANE_JSON_LOGGING` | `false` | JSON structured logging |

### xDS TLS (Optional)

| Variable | Description |
|----------|-------------|
| `FLOWPLANE_XDS_TLS_CERT_PATH` | Server certificate path |
| `FLOWPLANE_XDS_TLS_KEY_PATH` | Server private key path |
| `FLOWPLANE_XDS_TLS_CLIENT_CA_PATH` | Client CA for mTLS verification |
| `FLOWPLANE_XDS_TLS_REQUIRE_CLIENT_CERT` | Require client certificates (default: true if TLS enabled) |

### API TLS (Optional)

| Variable | Description |
|----------|-------------|
| `FLOWPLANE_API_TLS_ENABLED` | Enable HTTPS for API |
| `FLOWPLANE_API_TLS_CERT_PATH` | API server certificate |
| `FLOWPLANE_API_TLS_KEY_PATH` | API server private key |
| `FLOWPLANE_API_TLS_CHAIN_PATH` | Certificate chain (optional) |

### Secrets (Vault Integration)

| Variable | Description |
|----------|-------------|
| `FLOWPLANE_SECRET_BOOTSTRAP_TOKEN` | Initial setup token |
| `VAULT_ADDR` | Vault server address |
| `VAULT_TOKEN` | Vault authentication token |
| `VAULT_NAMESPACE` | Vault namespace |
| `FLOWPLANE_VAULT_PKI_MOUNT_PATH` | Vault PKI mount for mTLS |
| `FLOWPLANE_VAULT_PKI_ROLE_NAME` | Vault PKI role |
| `FLOWPLANE_VAULT_PKI_TRUST_DOMAIN` | SPIFFE trust domain |

## Example Configurations

### Development

```bash
export FLOWPLANE_DATABASE_URL=postgresql://flowplane:flowplane@localhost:5432/flowplane
export FLOWPLANE_API_PORT=8080
export FLOWPLANE_XDS_PORT=18000
export RUST_LOG=info,flowplane=debug
```

### Production

```bash
export FLOWPLANE_DATABASE_URL=postgresql://flowplane:password@postgres:5432/flowplane?sslmode=require
export FLOWPLANE_API_BIND_ADDRESS=0.0.0.0
export FLOWPLANE_API_PORT=8080
export FLOWPLANE_API_TLS_ENABLED=true
export FLOWPLANE_API_TLS_CERT_PATH=/etc/flowplane/certs/api-cert.pem
export FLOWPLANE_API_TLS_KEY_PATH=/etc/flowplane/certs/api-key.pem
export FLOWPLANE_XDS_BIND_ADDRESS=0.0.0.0
export FLOWPLANE_XDS_PORT=50051
export FLOWPLANE_XDS_TLS_CERT_PATH=/etc/flowplane/certs/xds-server.pem
export FLOWPLANE_XDS_TLS_KEY_PATH=/etc/flowplane/certs/xds-server.key
export FLOWPLANE_XDS_TLS_CLIENT_CA_PATH=/etc/flowplane/certs/xds-ca.pem
export FLOWPLANE_ENABLE_METRICS=true
export FLOWPLANE_ENABLE_TRACING=true
export FLOWPLANE_SERVICE_NAME=flowplane-control-plane
export RUST_LOG=info,flowplane=info,sqlx=warn
```

See [Operations Guide](operations.md) for production deployment details.
