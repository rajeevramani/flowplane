# Flowplane v0.0.1 Release Notes

**Release Date:** 2025-10-07

## Overview

Flowplane v0.0.1 is the initial release of our Envoy control plane, providing a RESTful interface for managing Envoy proxy configurations through structured Rust/JSON models. This release delivers a production-ready foundation for API gateway operations with comprehensive authentication, TLS support, and filter management.

## 🎯 Key Features

### Core Control Plane
- **xDS Server**: Full implementation of Envoy's xDS protocol (ADS, LDS, RDS, CDS, EDS)
- **SQLite Backend**: Embedded database for configuration persistence
- **HTTP REST API**: Comprehensive API for cluster, route, and listener management
- **OpenAPI Specification**: Auto-generated API documentation via `utoipa` with Swagger UI

### Authentication & Security
- **Personal Access Tokens**: Scoped token-based authentication for all API endpoints
- **Bootstrap Token**: Automatic generation of admin token on first startup with prominent display
- **Token Management**: Full CRUD operations with scope-based permissions
  - Scopes: `clusters:*`, `routes:*`, `listeners:*`, `tokens:*`, `gateways:import`
- **Audit Logging**: Comprehensive audit trail for all authentication and resource operations

### TLS Support
- **xDS TLS/mTLS**: Secure control plane ↔ data plane communication
- **API TLS**: Optional HTTPS termination for REST API
- **Certificate Management**: Support for PEM-encoded certificates and keys

### Gateway & Routing
- **Default Gateway**: Pre-seeded shared gateway resources for quick starts
- **OpenAPI Import**: One-call gateway creation from OpenAPI 3.0 specifications
- **Multi-Tenancy**: Listener isolation with per-team namespace support
- **Route Management**: Flexible routing with prefix, exact, and regex path matching
- **HTTP Method Filtering**: Envoy route generation with HTTP method constraints

### HTTP Filters
- **Rate Limiting**: Local rate limiting with token bucket algorithm
- **CORS**: First-class CORS policy configuration
- **Custom Response**: User-friendly custom error response configuration
- **Header Mutation**: Request/response header manipulation
- **Health Check**: Health check filter for graceful degradation
- **JWT Authentication**: JWT validation and claim extraction
- **Router**: Required terminal filter for request routing

### OpenAPI Extensions
- **x-flowplane Extensions**: Custom OpenAPI extensions for filter configuration
  - `x-flowplane-filters`: Global and per-route filter configuration
  - `x-flowplane-custom-response`: Simplified custom response syntax
- **Filter Overrides**: Route-level filter configuration overrides global settings

### CLI Tool
- **HTTP-Based Commands**: Complete CLI for all API operations
- **Token Management**: Token creation, rotation, and revocation
- **Resource Management**: CRUD operations for clusters, routes, and listeners
- **Database Operations**: Direct database migrations and management
- **Comprehensive Help**: Detailed help system with examples

### Observability
- **Structured Logging**: JSON-formatted logs with configurable levels
- **Prometheus Metrics**: Optional metrics export (configurable)
- **Tracing Support**: Optional OpenTelemetry tracing integration
- **Audit Logs**: Database-persisted audit trail for security events

### Deployment
- **Docker Support**: Production-ready Docker images for Control Plane and CLI
- **Docker Compose**: Complete local development environment with SQLite
- **Multi-stage Builds**: Optimized Docker images using Rust 1.89+
- **Health Checks**: Built-in health check endpoints for container orchestration

## 📦 Installation

### Docker (Recommended)

```bash
# Start Control Plane
docker-compose up -d

# Extract bootstrap token
docker-compose logs control-plane 2>&1 | grep -oP 'token: \Kfp_pat_[^\s]+'

# Access API
curl http://localhost:8080/swagger-ui/
```

See [README-DOCKER.md](README-DOCKER.md) for complete Docker documentation.

### From Source

```bash
# Prerequisites: Rust 1.75+, SQLite
cargo build --release --bin flowplane

# Run migrations
cargo run --bin run_migrations

# Start server
FLOWPLANE_XDS_PORT=50051 \
FLOWPLANE_DATABASE_URL=sqlite://./data/flowplane.db \
cargo run --bin flowplane
```

## 🔧 Configuration

### Environment Variables

**Core Settings:**
- `DATABASE_URL`: SQLite or PostgreSQL connection string (required)
- `FLOWPLANE_API_BIND_ADDRESS`: API server bind address (default: `127.0.0.1`)
- `FLOWPLANE_API_PORT`: API server port (default: `8080`)
- `FLOWPLANE_XDS_BIND_ADDRESS`: xDS server bind address (default: `0.0.0.0`)
- `FLOWPLANE_XDS_PORT`: xDS gRPC port (default: `50051`)

**TLS Settings:**
- `FLOWPLANE_XDS_TLS_CERT_PATH`: xDS server certificate
- `FLOWPLANE_XDS_TLS_KEY_PATH`: xDS server private key
- `FLOWPLANE_XDS_TLS_CLIENT_CA_PATH`: CA bundle for client validation
- `FLOWPLANE_API_TLS_ENABLED`: Enable HTTPS for API (default: `false`)

**Observability:**
- `RUST_LOG`: Logging level (default: `info`)
- `FLOWPLANE_ENABLE_METRICS`: Enable Prometheus metrics (default: `true`)
- `FLOWPLANE_ENABLE_TRACING`: Enable tracing (default: `false`)

## 📚 Documentation

- [Getting Started Guide](docs/getting-started.md) - Step-by-step tutorial
- [Architecture Overview](docs/architecture.md) - System design and components
- [API Reference](docs/api.md) - REST API documentation
- [Authentication](docs/authentication.md) - Token-based auth system
- [TLS Configuration](docs/tls.md) - TLS/mTLS setup
- [Filter Configuration](docs/filters.md) - HTTP filter reference
- [OpenAPI Extensions](examples/README-x-flowplane-extensions.md) - Custom extensions
- [Docker Guide](README-DOCKER.md) - Docker deployment

## 🧪 Testing

- **168 Unit Tests**: Comprehensive test coverage for core functionality
- **Integration Tests**: E2E tests with actual Envoy proxies
- **CI Pipeline**: Automated testing on every commit (GitHub Actions)
  - Cargo check, test, fmt, clippy
  - Security audit (cargo-audit)
  - E2E smoke tests
  - Full E2E suite on main branch

## 🔄 API Endpoints

### Authentication
- `POST /api/v1/tokens` - Create token
- `GET /api/v1/tokens` - List tokens
- `GET /api/v1/tokens/{id}` - Get token
- `PATCH /api/v1/tokens/{id}` - Update token
- `DELETE /api/v1/tokens/{id}` - Revoke token
- `POST /api/v1/tokens/{id}/rotate` - Rotate token

### Clusters
- `POST /api/v1/clusters` - Create cluster
- `GET /api/v1/clusters` - List clusters
- `GET /api/v1/clusters/{name}` - Get cluster
- `PUT /api/v1/clusters/{name}` - Update cluster
- `DELETE /api/v1/clusters/{name}` - Delete cluster

### Routes
- `POST /api/v1/routes` - Create route
- `GET /api/v1/routes` - List routes
- `GET /api/v1/routes/{name}` - Get route
- `PUT /api/v1/routes/{name}` - Update route
- `DELETE /api/v1/routes/{name}` - Delete route

### Listeners
- `POST /api/v1/listeners` - Create listener
- `GET /api/v1/listeners` - List listeners
- `GET /api/v1/listeners/{name}` - Get listener
- `PUT /api/v1/listeners/{name}` - Update listener
- `DELETE /api/v1/listeners/{name}` - Delete listener

### API Definitions (Gateway API)
- `POST /api/v1/api-definitions` - Create BFF API definition
- `POST /api/v1/api-definitions/import-openapi` - Import OpenAPI spec
- `POST /api/v1/api-definitions/{name}/routes` - Append route to API
- `GET /api/v1/api-definitions` - List API definitions
- `GET /api/v1/api-definitions/{name}` - Get API definition
- `GET /api/v1/bootstrap` - Get bootstrap resources

## 🐛 Known Issues

1. **E2E Tests**: Some E2E tests marked as `|| true` due to Envoy installation variability in CI
2. **PostgreSQL**: While supported, primary testing uses SQLite
3. **Filter Coverage**: Not all Envoy filters are implemented yet (see [FILTER_COVERAGE_AUDIT.md](.local/docs/FILTER_COVERAGE_AUDIT.md))

## 🔜 Roadmap

### Planned for v0.1.0
- Additional HTTP filters (ext_authz, RBAC, WASM)
- PostgreSQL production hardening
- Metrics dashboard
- Enhanced CLI with interactive mode
- Kubernetes operator

### Future Considerations
- gRPC support
- A2A (Application-to-Application) protocol
- MCP (Model Context Protocol) support
- GraphQL gateway capabilities
- Policy engine integration

## 🤝 Contributing

We welcome contributions! Please see [CONTRIBUTING.md](docs/contributing.md) for guidelines.

### Development Setup

```bash
# Clone repository
git clone https://github.com/rajeevramani/flowplane.git
cd flowplane

# Run tests
cargo test --all-features

# Run clippy
cargo clippy --all-targets --all-features -- -D warnings

# Format code
cargo fmt --all

# Run E2E tests (requires Envoy)
RUN_E2E=1 cargo test --test smoke_boot_and_route -- --ignored --nocapture
```

## 📄 License

See [LICENSE](LICENSE) file for details.

## 🙏 Acknowledgments

- **Envoy Community**: For the excellent proxy and xDS protocol
- **envoy-types**: Rust bindings for Envoy protobufs
- **Rust Ecosystem**: axum, tokio, tonic, sqlx, and many others

## 📞 Support

- **Documentation**: https://github.com/rajeevramani/flowplane/tree/main/docs
- **Issues**: https://github.com/rajeevramani/flowplane/issues
- **Discussions**: https://github.com/rajeevramani/flowplane/discussions

---

**Flowplane** - *Smooth traffic flow through your control plane* (प्रवाह)
