# Flowplane Docker Compose Configurations

This document describes the Docker Compose configurations available for running Flowplane with various features enabled.

## Table of Contents

- [Quick Start](#quick-start)
- [Available Configurations](#available-configurations)
- [Configuration Files](#configuration-files)
- [Testing Scenarios](#testing-scenarios)
- [Troubleshooting](#troubleshooting)

## Quick Start

### Prerequisites

- Docker Engine 20.10+ and Docker Compose 2.0+
- At least 4GB of available RAM
- Ports 8080, 9411, 10000, 50051 available (varies by configuration)

### Basic Setup

1. **Clone the repository and navigate to the project root**

2. **Create environment configuration** (optional):
   ```bash
   cp .env.example .env
   # Edit .env and set BOOTSTRAP_TOKEN (minimum 32 characters)
   ```

3. **Choose a configuration and start services**:
   ```bash
   # For tracing only
   docker-compose -f docker-compose-zipkin.yml up

   # For secrets + tracing
   docker-compose -f docker-compose-secrets-tracing.yml up
   ```

4. **Access the services**:
   - Flowplane API: http://localhost:8080
   - Swagger UI: http://localhost:8080/swagger-ui/
   - Zipkin UI: http://localhost:9411
   - Envoy Admin: http://localhost:9901
   - httpbin: http://localhost:8000

## Available Configurations

### 1. docker-compose.yml (Basic)

**Purpose**: Minimal Flowplane control plane without observability features

**Services**:
- Flowplane Control Plane

**Use Cases**:
- Quick local testing
- Minimal resource usage
- Development without tracing overhead

**Start**:
```bash
docker-compose up
```

---

### 2. docker-compose-zipkin.yml (Tracing)

**Purpose**: Flowplane with distributed tracing enabled using Zipkin

**Services**:
- Flowplane Control Plane (with tracing)
- Zipkin server
- Envoy proxy (xDS-configured)
- httpbin (for testing)

**Features**:
- OpenTelemetry tracing with OTLP exporter
- Zipkin UI for trace visualization
- W3C TraceContext propagation
- 100% trace sampling (development mode)

**Start**:
```bash
docker-compose -f docker-compose-zipkin.yml up
```

**Access**:
- Zipkin UI: http://localhost:9411
- Flowplane API: http://localhost:8080
- Envoy proxy: http://localhost:10000
- httpbin: http://localhost:8000

---

### 3. docker-compose-secrets-tracing.yml (Full)

**Purpose**: Complete Flowplane setup with secrets management and tracing

**Services**:
- Flowplane Control Plane (with secrets + tracing)
- HashiCorp Vault (dev mode)
- Zipkin server
- Envoy proxy (xDS-configured)
- httpbin (for testing)

**Features**:
- Secrets management with automatic environment-based selection:
  - **Vault mode** (when `VAULT_ADDR` and `VAULT_TOKEN` are set): Bootstrap token stored in Vault, rotation API enabled
  - **Development mode** (when Vault env vars not set): Bootstrap token from environment only, no rotation
- OpenTelemetry tracing
- Full observability stack
- Production-like configuration

**Start**:
```bash
docker-compose -f docker-compose-secrets-tracing.yml up
```

**Access**:
- Vault UI: http://localhost:8200 (token: `flowplane-dev-token`)
- Zipkin UI: http://localhost:9411
- Flowplane API: http://localhost:8080
- Envoy proxy: http://localhost:10000
- httpbin: http://localhost:8000

## Configuration Files

### Environment Variables

All Docker Compose files support environment variable configuration via `.env` file:

```bash
cp .env.example .env
# Edit .env with your configuration
```

**Key Variables**:

| Variable | Description | Default | Required |
|----------|-------------|---------|----------|
| `BOOTSTRAP_TOKEN` | Admin authentication token | auto-generated | Yes (min 32 chars) |
| `VAULT_ADDR` | Vault server address (enables Vault mode) | - | No |
| `VAULT_TOKEN` | Vault authentication token | - | No |
| `FLOWPLANE_ENABLE_TRACING` | Enable distributed tracing | `true` | No |
| `FLOWPLANE_OTLP_ENDPOINT` | OTLP exporter endpoint | `http://zipkin:9411/api/v2/spans` | No |
| `FLOWPLANE_TRACE_SAMPLING_RATIO` | Trace sampling ratio (0.0-1.0) | `1.0` | No |
| `RUST_LOG` | Log level configuration | `info,flowplane=debug` | No |

**Secrets Management Modes**:

Flowplane automatically selects the secrets backend based on environment variables:

- **Vault Mode** (Production): When both `VAULT_ADDR` and `VAULT_TOKEN` are set
  - Bootstrap token stored in Vault at `secret/bootstrap_token`
  - Bootstrap token rotation API enabled
  - Provides audit trail and secret versioning

- **Development Mode**: When `VAULT_ADDR` or `VAULT_TOKEN` are not set
  - Bootstrap token read from `BOOTSTRAP_TOKEN` environment variable
  - Token stored hashed in database only
  - No rotation support

See `.env.example` for complete list.

### Envoy Bootstrap Configuration

The `envoy-bootstrap.yaml` file configures Envoy to connect to Flowplane's xDS server:

- **Node ID**: `envoy-test-node`
- **Cluster**: `flowplane-test-cluster`
- **Team**: `default` (for RBAC isolation)
- **xDS Server**: `control-plane:50051`

**Dynamic Resources** (configured via xDS):
- Listeners (LDS)
- Routes (RDS)
- Clusters (CDS)
- Endpoints (EDS)

## Testing Scenarios

### 1. Basic Health Check

Verify all services are running:

```bash
# Check Flowplane API
curl http://localhost:8080/swagger-ui/

# Check Zipkin
curl http://localhost:9411/health

# Check Envoy admin
curl http://localhost:9901/stats

# Check httpbin
curl http://localhost:8000/get
```

### 2. Test Distributed Tracing

1. **Start services**:
   ```bash
   docker-compose -f docker-compose-zipkin.yml up -d
   ```

2. **Create a listener and route** (example using API):
   ```bash
   # Get bootstrap token from logs
   BOOTSTRAP_TOKEN=$(docker-compose -f docker-compose-zipkin.yml logs control-plane | grep "fp_pat_" | awk '{print $NF}')

   # Create listener via API (requires authentication - implement as needed)
   # See Flowplane API documentation for endpoint details
   ```

3. **Send test requests through Envoy**:
   ```bash
   curl http://localhost:10000/get
   ```

4. **View traces in Zipkin UI**:
   - Open http://localhost:9411
   - Click "Run Query" to see recent traces
   - Traces show: API request → xDS update → Envoy config → httpbin response

### 3. Test Secrets Management

#### Vault Mode (Production-like)

1. **Start services with Vault enabled**:
   ```bash
   # Vault mode is enabled by default in docker-compose-secrets-tracing.yml
   # VAULT_ADDR and VAULT_TOKEN are set in the environment
   docker-compose -f docker-compose-secrets-tracing.yml up -d
   ```

2. **Verify bootstrap token was stored in Vault**:
   ```bash
   # Check Flowplane logs for Vault connection
   docker-compose -f docker-compose-secrets-tracing.yml logs control-plane | grep -i "vault\|secrets backend"

   # Expected output:
   # ✓ Successfully connected to Vault, address: http://vault:8200
   # ✓ Using Vault secrets backend for bootstrap token storage and rotation
   # ✓ Stored bootstrap token in secrets backend for future rotation support
   ```

3. **Read the bootstrap token from Vault**:
   ```bash
   docker-compose -f docker-compose-secrets-tracing.yml exec -e VAULT_TOKEN=flowplane-dev-token vault \
     vault kv get secret/bootstrap_token

   # Output shows:
   # Key      Value
   # ---      -----
   # value    <your-bootstrap-token-secret>
   ```

4. **Test token rotation** (requires Vault mode):
   ```bash
   # Rotate the bootstrap token via API
   # This will generate a new secret in Vault and update the database
   curl -X POST http://localhost:8080/api/v1/auth/tokens/bootstrap/rotate \
     -H "Authorization: Bearer <your-current-bootstrap-token>"
   ```

#### Development Mode (No Vault)

1. **Disable Vault by removing environment variables**:
   ```bash
   # Edit docker-compose-secrets-tracing.yml
   # Comment out or remove these lines:
   # VAULT_ADDR: http://vault:8200
   # VAULT_TOKEN: flowplane-dev-token
   ```

2. **Restart services**:
   ```bash
   docker-compose -f docker-compose-secrets-tracing.yml up -d
   ```

3. **Verify development mode**:
   ```bash
   docker-compose -f docker-compose-secrets-tracing.yml logs control-plane | grep "secrets backend"

   # Expected output:
   # ✓ No Vault configuration detected (VAULT_ADDR/VAULT_TOKEN not set)
   # ✓ Using environment variable only mode for development
   # ✓ Bootstrap token rotation will not be available
   ```

### 4. Test xDS Dynamic Configuration

1. **Monitor Envoy config**:
   ```bash
   # Watch Envoy config updates
   watch -n 1 'curl -s http://localhost:9901/config_dump | jq ".configs[].\"@type\""'
   ```

2. **Create/update resources via Flowplane API**:
   - Listeners, routes, clusters will be dynamically pushed to Envoy
   - Envoy will update configuration without restart

3. **Verify changes in Envoy**:
   ```bash
   curl http://localhost:9901/config_dump | jq '.configs[] | select(.["@type"] | contains("Listener"))'
   ```

## Troubleshooting

### Services Not Starting

**Check logs**:
```bash
docker-compose -f docker-compose-zipkin.yml logs control-plane
docker-compose -f docker-compose-zipkin.yml logs zipkin
docker-compose -f docker-compose-zipkin.yml logs envoy
```

**Common issues**:

1. **Port conflicts**: Ensure ports 8080, 9411, 10000, 50051 are available
   ```bash
   # Check port usage
   lsof -i :8080
   lsof -i :9411
   ```

2. **BOOTSTRAP_TOKEN not set or too short**:
   ```
   Error: BOOTSTRAP_TOKEN must be at least 32 characters
   ```
   Solution: Set `BOOTSTRAP_TOKEN` in `.env` file with minimum 32 characters

3. **Docker resource limits**:
   ```bash
   # Increase Docker memory to 4GB minimum
   # Docker Desktop: Settings → Resources → Memory
   ```

### Tracing Not Working

1. **Check Zipkin connectivity**:
   ```bash
   docker-compose -f docker-compose-zipkin.yml exec control-plane \
     curl -v http://zipkin:9411/health
   ```

2. **Verify tracing is enabled**:
   ```bash
   docker-compose -f docker-compose-zipkin.yml logs control-plane | grep -i "tracing"
   ```

3. **Check OTLP endpoint configuration**:
   ```bash
   docker-compose -f docker-compose-zipkin.yml exec control-plane \
     env | grep FLOWPLANE_OTLP_ENDPOINT
   ```

### Envoy Not Connecting to xDS

1. **Check Envoy logs**:
   ```bash
   docker-compose -f docker-compose-zipkin.yml logs envoy | grep -i "xds\|grpc"
   ```

2. **Verify control plane is reachable**:
   ```bash
   docker-compose -f docker-compose-zipkin.yml exec envoy \
     nc -zv control-plane 50051
   ```

3. **Check node metadata**:
   ```bash
   # Ensure team metadata is set in envoy-bootstrap.yaml
   cat envoy-bootstrap.yaml | grep -A 2 metadata
   ```

### Vault Access Issues

1. **Verify Vault is running**:
   ```bash
   curl http://localhost:8200/v1/sys/health
   ```

2. **Check Vault token**:
   ```bash
   docker-compose -f docker-compose-secrets-tracing.yml logs vault | grep "Root Token"
   ```

3. **Test Vault CLI**:
   ```bash
   docker-compose -f docker-compose-secrets-tracing.yml exec vault \
     vault status
   ```

## Cleanup

### Stop and remove containers:
```bash
# For specific configuration
docker-compose -f docker-compose-zipkin.yml down

# Remove volumes (WARNING: deletes data)
docker-compose -f docker-compose-zipkin.yml down -v
```

### Clean up Docker resources:
```bash
# Remove unused images
docker image prune -a

# Remove unused volumes
docker volume prune
```

## Production Considerations

The Docker Compose configurations provided are designed for **development and testing**. For production deployments:

1. **Security**:
   - Use secure token generation (not defaults)
   - Enable TLS for all services
   - Use production-grade Vault (not dev mode)
   - Configure proper RBAC and authentication

2. **Persistence**:
   - Use external database (PostgreSQL)
   - Configure persistent volumes
   - Set up backup strategies

3. **Observability**:
   - Adjust trace sampling ratio (e.g., 0.1 for 10%)
   - Configure metrics export to Prometheus
   - Set up log aggregation

4. **High Availability**:
   - Run multiple control plane instances
   - Use external load balancer
   - Configure health checks and auto-restart

5. **Resource Limits**:
   - Set CPU and memory limits for each service
   - Configure appropriate restart policies
   - Monitor resource usage

## Additional Resources

- [Flowplane Documentation](../README.md)
- [Architecture Overview](../docs/architecture.md)
- [API Documentation](../docs/api.md)
- [Getting Started Guide](../docs/getting-started.md)
- [Envoy xDS Protocol](https://www.envoyproxy.io/docs/envoy/latest/api-docs/xds_protocol)
- [OpenTelemetry Tracing](https://opentelemetry.io/docs/concepts/signals/traces/)
- [HashiCorp Vault](https://www.vaultproject.io/docs)
- [Zipkin](https://zipkin.io/pages/quickstart)
