# Flowplane v0.0.2 Release Notes

**Release Date:** 2025-10-12

## Overview

Flowplane v0.0.2 delivers major security enhancements with token-based RBAC and team isolation, API definition management improvements with PATCH support, comprehensive route flows reporting, and critical bug fixes. This release significantly strengthens multi-tenancy capabilities while improving operational visibility and API usability.

## 🎯 Key Features

### Security & RBAC

- **Token-Based RBAC**: Complete role-based access control with team scoping
  - Team-scoped tokens can only access their team's resources
  - Database-level filtering prevents data leakage across teams
  - `admin:all` scope grants full cross-team access
  - Added `team` column to clusters, routes, listeners, and api_definitions tables

- **Required Bootstrap Token**: Environment-based bootstrap token configuration
  - Bootstrap token must be provided via `BOOTSTRAP_TOKEN` environment variable
  - Minimum 32 characters enforced with secure token generation guidance
  - Prevents use of default/example tokens
  - Idempotent behavior - token created once on first boot

- **Dynamic Scope Derivation**: HTTP method-based scope requirements
  - GET operations require `:read` scopes
  - POST/PUT/PATCH/DELETE require `:write` scopes
  - Automatic scope validation middleware

### API Definition Management

- **PATCH Endpoint**: Partial updates for API definitions
  - Endpoint: `PATCH /api/v1/api-definitions/{id}`
  - Supported fields: `domain`, `tls`, `targetListeners`
  - Version auto-increment on successful updates
  - Validation ensures at least one field is provided

- **xDS Cache Refresh Integration**: Automatic Envoy configuration updates
  - Changes propagate immediately to connected Envoy proxies
  - Mode-aware refresh ordering (isolated vs shared listeners)
  - Proper xDS update ordering: clusters → routes → platform API → listeners

- **Enhanced Bootstrap Response**: Detailed listener information
  - Endpoint: `GET /api/v1/api-definitions/{id}/bootstrap`
  - Returns listener details: name, address, port, protocol
  - Supports both isolated and shared listener modes
  - Example response:
    ```json
    {
      "listeners": [
        {
          "name": "default-gateway-listener",
          "address": "0.0.0.0",
          "port": 10000,
          "protocol": "HTTP"
        }
      ],
      "admin": { ... },
      ...
    }
    ```

### Reporting & Visibility

- **Route Flows Reporting**: New reporting API for request flow analysis
  - Endpoint: `GET /api/v1/reports/route-flows`
  - Returns comprehensive route flow data: listener → route → cluster → endpoint
  - Team-scoped filtering (admin tokens see all, team tokens see only their routes)
  - Pagination support (limit/offset parameters)
  - Requires `reports:read` scope

### Developer Experience

- **HTTP Test Examples**: Comprehensive .http files for API testing
  - Located in `.http-examples/` directory
  - Files: auth, clusters, routes, listeners, api-definitions, reporting, cleanup
  - VSCode REST Client integration
  - Environment variable support via .env file
  - Complete CRUD operation examples with documentation

- **.env File Loading**: Automatic environment file loading with dotenvy
  - Loads `.env` file at application startup
  - Optional file (won't fail if missing)
  - Supports `BOOTSTRAP_TOKEN` and `API_TOKEN` variables

### HTTP Filters

- **External Processing Filter**: New ext_proc filter support
  - Allows external HTTP service for request/response processing
  - Configurable processing modes and timeout settings
  - Integration with Envoy's external processing protocol

## 🐛 Critical Bug Fixes

### API Definition Bugs (PR #27)

- **OpenAPI Import Port Parameter**: Fixed port parameter being ignored during OpenAPI import
  - Resolved variable shadowing bug (url_port vs port)
  - Now uses provided port or generates from domain hash as fallback

- **Pagination Parameters**: Fixed limit/offset parameters ignored in list API definitions
  - Implemented proper LIMIT/OFFSET SQL clauses
  - Parameters now correctly passed from handler to repository

- **Team Filter Parameter**: Fixed team filter ignored in list API definitions
  - Added WHERE clause for team filtering
  - Uses parameterized queries for security

- **Token Expiry Security Issue**: Fixed tokens with no expiration
  - Applies default 30-day expiry when expires_at is None
  - Prevents security risk from perpetual tokens

### xDS Propagation Fix

- **PATCH Updates Not Propagating**: Fixed critical issue where PATCH operations didn't update Envoy
  - Resolved xDS refresh ordering issue
  - Domain and route updates now correctly propagate to data plane
  - Route cascade updates working properly

### Test Infrastructure

- **Scope Validation Regex**: Updated to support hyphens in resource names
  - Now supports `api-definitions:read` and other hyphenated resources
  - Comprehensive tests for all scope patterns

- **Test Fixtures**: Updated all test fixtures with new `team` field
  - Fixed compilation errors in platform_api and database constraint tests
  - All 331 unit tests passing

## 📦 Installation

### Upgrading from v0.0.1

**IMPORTANT: v0.0.2 requires a `BOOTSTRAP_TOKEN` environment variable**

```bash
# Generate secure bootstrap token
export BOOTSTRAP_TOKEN=$(openssl rand -base64 32)

# Docker users
docker pull rajeevramani/flowplane:v0.0.2
docker-compose down
# Add BOOTSTRAP_TOKEN to docker-compose.yml or .env
docker-compose up -d

# Binary users
wget https://github.com/rajeevramani/flowplane/releases/download/v0.0.2/flowplane-<platform>.tar.gz
tar xzf flowplane-<platform>.tar.gz
./flowplane # Requires BOOTSTRAP_TOKEN in environment
```

### Fresh Installation

See [v0.0.1 Release Notes](RELEASE_NOTES_v0.0.1.md) for detailed installation instructions. Add `BOOTSTRAP_TOKEN` environment variable as shown above.

## 🔧 Configuration Changes

### New Required Environment Variables

- **`BOOTSTRAP_TOKEN`** (REQUIRED): Bootstrap token for initial admin access
  - Minimum 32 characters
  - Generate with: `openssl rand -base64 32`
  - Must be set before first startup

### Optional Environment Variables

- **`API_TOKEN`**: For HTTP test examples integration

## 🔄 API Changes

### New Endpoints

- **`PATCH /api/v1/api-definitions/{id}`** - Partial update of API definition
  - Requires: `api-definitions:write` scope
  - Body fields: `domain`, `tls`, `targetListeners` (all optional, at least one required)
  - Returns: Updated API definition
  - Automatically triggers xDS cache refresh

- **`GET /api/v1/reports/route-flows`** - Route flows reporting
  - Requires: `reports:read` scope
  - Query parameters: `limit`, `offset`, `team` (optional)
  - Returns: List of route flows with pagination

### Enhanced Endpoints

- **`GET /api/v1/api-definitions/{id}/bootstrap`** - Now includes listener details
  - Added `listeners` array with name, address, port, protocol
  - Enhanced for both isolated and shared listener modes

### Modified Scope Requirements

- API definition endpoints now require `api-definitions:read` and `api-definitions:write` scopes
- Report endpoints require `reports:read` scope
- Admin operations require `admin:all` scope for cross-team access

## 🧪 Testing

- **331 Unit Tests**: All passing
- **52 CLI Integration Tests**: All passing
- **27 Platform API Tests**: All passing
- **Enhanced E2E Suite**: PATCH operations, configuration propagation, filter chain execution
- **CI Pipeline**: All checks passing ✓

## 📚 API Examples

### PATCH API Definition

```bash
# Update domain only
curl -X PATCH http://localhost:8080/api/v1/api-definitions/my-api \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"domain": "api.newdomain.com"}'

# Update target listeners
curl -X PATCH http://localhost:8080/api/v1/api-definitions/my-api \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"targetListeners": ["listener-1", "listener-2"]}'
```

### Get Bootstrap with Listener Info

```bash
curl http://localhost:8080/api/v1/api-definitions/my-api/bootstrap \
  -H "Authorization: Bearer $TOKEN"
```

### Get Route Flows Report

```bash
# Get first 10 route flows
curl http://localhost:8080/api/v1/reports/route-flows?limit=10 \
  -H "Authorization: Bearer $TOKEN"

# Get route flows for specific team
curl http://localhost:8080/api/v1/reports/route-flows?team=platform \
  -H "Authorization: Bearer $TOKEN"
```

## 🔐 Migration Notes

### Database Migrations

New migration: `20251010000001_add_team_columns_for_rbac.sql`
- Adds `team` column to clusters, routes, listeners, api_definitions tables
- Automatically applied on startup with existing database
- No manual intervention required

### Token Migration

- Existing tokens from v0.0.1 continue to work
- Bootstrap token must be set in environment for new deployments
- Consider rotating tokens to add team scopes for RBAC

## 📊 Compatibility

- **API Compatibility**: Fully backward compatible with v0.0.1 (except new required `BOOTSTRAP_TOKEN`)
- **Database Compatibility**: Automatic migration from v0.0.1 schema
- **Configuration Compatibility**: All v0.0.1 configs work (with added `BOOTSTRAP_TOKEN`)
- **Envoy Compatibility**: Tested with Envoy 1.27+

## 🔜 What's Next

### Planned for v0.0.3

- Enhanced OpenAPI x-flowplane extension capabilities
- Additional reporting endpoints and metrics
- Performance optimizations for team-scoped queries
- Configuration validation dry-run mode

### Under Consideration

- GraphQL support for API definitions
- Configuration versioning and rollback
- Audit log reporting endpoints
- Policy engine integration

## 🔗 Pull Requests in This Release

- #26 - feat: Add route flows reporting and HTTP test examples
- #27 - Fix critical API bugs and test infrastructure
- Individual commits for PATCH endpoint, xDS refresh, and bootstrap enhancements

## 📚 Documentation

### New Documentation

- **Database Evaluation**: [.local/docs/database-evaluation.md](.local/docs/database-evaluation.md)
  - Comprehensive SQLite vs PostgreSQL analysis
  - Performance characteristics and scalability recommendations
  - Migration roadmap for PostgreSQL support in v0.1.0

### HTTP Test Examples

- **Getting Started**: [.http-examples/README.md](.http-examples/README.md)
  - VSCode REST Client setup
  - Environment variable configuration
  - Complete API testing workflow

## 🐛 Known Issues

1. **Route Updates in PATCH**: Full route replacement not yet supported in PATCH endpoint (use PUT for now)
2. **E2E Test Stability**: Some E2E tests may be flaky in CI environments with Envoy installation issues

## 🤝 Contributing

We welcome contributions! Please see [CONTRIBUTING.md](docs/contributing.md) for guidelines.

### Development Setup

```bash
# Clone and checkout v0.0.2
git clone https://github.com/rajeevramani/flowplane.git
cd flowplane
git checkout v0.0.2

# Set bootstrap token for tests
export BOOTSTRAP_TOKEN=$(openssl rand -base64 32)

# Run tests
cargo test --all-features
cargo clippy --all-targets --all-features
cargo fmt --all

# Run E2E tests (requires Envoy)
RUN_E2E=1 cargo test --test smoke_boot_and_route -- --ignored --nocapture
```

## 📄 License

See [LICENSE](LICENSE) file for details.

## 🙏 Acknowledgments

Thanks to all contributors and the Rust/Envoy communities for their continued support.

## 📞 Support

- **Documentation**: https://github.com/rajeevramani/flowplane/tree/main/docs
- **Issues**: https://github.com/rajeevramani/flowplane/issues
- **Discussions**: https://github.com/rajeevramani/flowplane/discussions

---

**Flowplane** - *Smooth traffic flow through your control plane* (प्रवाह)
