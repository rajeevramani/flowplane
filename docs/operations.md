# Operations Guide

This guide covers production deployment, monitoring, scaling, and operational best practices for Flowplane control plane.

## Production Deployment

### Deployment Architecture

**Recommended Setup:**

```
┌─────────────────────────────────────────────────────┐
│                 Load Balancer                       │
│          (HAProxy / NGINX / ALB)                    │
└────────────┬────────────────────┬───────────────────┘
             │                    │
    ┌────────▼────────┐  ┌───────▼─────────┐
    │ Flowplane CP #1 │  │ Flowplane CP #2 │
    │   (Active)      │  │   (Standby)     │
    │   :8080 API     │  │   :8080 API     │
    │   :50051 xDS    │  │   :50051 xDS    │
    └────────┬────────┘  └───────┬─────────┘
             │                    │
             └────────┬───────────┘
                      │
             ┌────────▼────────┐
             │   PostgreSQL    │
             │   (Primary +    │
             │    Read Replicas)│
             └─────────────────┘
```

**Key Components:**
- **Load Balancer**: Distributes API traffic, handles TLS termination (optional)
- **Multiple Control Plane Instances**: High availability, rolling updates
- **PostgreSQL**: Shared configuration state with replication
- **Envoy Proxies**: Connect via xDS to any control plane instance

### Environment Configuration

#### Production Environment Variables

```bash
# Database (PostgreSQL recommended for production)
DATABASE_URL=postgresql://flowplane:password@postgres:5432/flowplane?sslmode=require

# API Server
FLOWPLANE_API_BIND_ADDRESS=0.0.0.0
FLOWPLANE_API_PORT=8080
FLOWPLANE_API_TLS_ENABLED=true
FLOWPLANE_API_TLS_CERT_PATH=/etc/flowplane/certs/api-cert.pem
FLOWPLANE_API_TLS_KEY_PATH=/etc/flowplane/certs/api-key.pem
FLOWPLANE_API_TLS_CHAIN_PATH=/etc/flowplane/certs/api-chain.pem

# xDS Server (with mTLS)
FLOWPLANE_XDS_BIND_ADDRESS=0.0.0.0
FLOWPLANE_XDS_PORT=50051
FLOWPLANE_XDS_TLS_CERT_PATH=/etc/flowplane/certs/xds-server.pem
FLOWPLANE_XDS_TLS_KEY_PATH=/etc/flowplane/certs/xds-server.key
FLOWPLANE_XDS_TLS_CLIENT_CA_PATH=/etc/flowplane/certs/xds-ca.pem
FLOWPLANE_XDS_TLS_REQUIRE_CLIENT_CERT=true

# Observability
RUST_LOG=info,flowplane=info,sqlx=warn
FLOWPLANE_ENABLE_METRICS=true
FLOWPLANE_ENABLE_TRACING=true
FLOWPLANE_SERVICE_NAME=flowplane-control-plane
FLOWPLANE_JAEGER_ENDPOINT=http://jaeger:14268/api/traces
```

#### Security Hardening

**1. Token Management:**
```bash
# On first deployment, extract setup token from logs
SETUP_TOKEN=$(docker logs flowplane-cp 2>&1 | grep -oP 'fp_setup_[^\s]+')

# Exchange setup token for admin token via bootstrap endpoint
ADMIN_TOKEN=$(curl -sS -X POST http://flowplane:8080/api/v1/bootstrap/initialize \
  -H "Content-Type: application/json" \
  -d "{
    \"setupToken\": \"$SETUP_TOKEN\",
    \"adminTokenName\": \"admin-token\",
    \"adminTokenDescription\": \"Production admin token\"
  }" | jq -r '.adminToken')

# Store admin token securely (e.g., in secrets manager)
echo "$ADMIN_TOKEN" | vault kv put secret/flowplane/admin-token value=-

# Create service tokens with minimal scopes
curl -X POST http://flowplane:8080/api/v1/tokens \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "envoy-readonly",
    "scopes": ["team:production:listeners:read", "team:production:routes:read", "team:production:clusters:read"],
    "expiresAt": "2026-01-01T00:00:00Z"
  }'

# Note: Setup token is automatically revoked after successful bootstrap
```

**Setup Token Security Features (v0.0.6):**
- **Single-use**: Automatically revoked after successful bootstrap
- **Time-limited**: Expires after 24 hours
- **Lockout protection**: Auto-locks after 5 failed attempts
- **Audit logging**: All operations logged to audit_log table

**2. TLS Certificates:**

Use proper certificates for production (not self-signed):
- **Let's Encrypt** for public-facing APIs
- **Corporate PKI** for internal deployments
- **Cert-Manager** for Kubernetes environments

See [tls.md](tls.md) for certificate generation workflows.

**3. Network Security:**

```bash
# Firewall rules (iptables example)
# Allow API traffic only from trusted networks
iptables -A INPUT -p tcp --dport 8080 -s 10.0.0.0/8 -j ACCEPT
iptables -A INPUT -p tcp --dport 8080 -j DROP

# Allow xDS traffic from Envoy proxies
iptables -A INPUT -p tcp --dport 50051 -s 10.0.1.0/24 -j ACCEPT
iptables -A INPUT -p tcp --dport 50051 -j DROP
```

### Docker Deployment

**Production docker-compose.yml:**

```yaml
version: '3.8'

services:
  postgres:
    image: postgres:15-alpine
    environment:
      POSTGRES_DB: flowplane
      POSTGRES_USER: flowplane
      POSTGRES_PASSWORD_FILE: /run/secrets/db_password
    volumes:
      - postgres_data:/var/lib/postgresql/data
      - ./backups:/backups
    secrets:
      - db_password
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U flowplane"]
      interval: 10s
      timeout: 5s
      retries: 5

  flowplane-cp:
    image: flowplane:v0.0.1
    environment:
      DATABASE_URL: postgresql://flowplane:${DB_PASSWORD}@postgres:5432/flowplane
      FLOWPLANE_API_BIND_ADDRESS: 0.0.0.0
      FLOWPLANE_API_TLS_ENABLED: "true"
      FLOWPLANE_API_TLS_CERT_PATH: /certs/api-cert.pem
      FLOWPLANE_API_TLS_KEY_PATH: /certs/api-key.pem
      FLOWPLANE_XDS_TLS_CERT_PATH: /certs/xds-server.pem
      FLOWPLANE_XDS_TLS_KEY_PATH: /certs/xds-server.key
      FLOWPLANE_XDS_TLS_CLIENT_CA_PATH: /certs/xds-ca.pem
      RUST_LOG: info
      FLOWPLANE_ENABLE_METRICS: "true"
    volumes:
      - ./certs:/certs:ro
    ports:
      - "8080:8080"
      - "50051:50051"
    depends_on:
      postgres:
        condition: service_healthy
    restart: unless-stopped
    deploy:
      resources:
        limits:
          cpus: '2'
          memory: 1G
        reservations:
          cpus: '0.5'
          memory: 256M

  prometheus:
    image: prom/prometheus:latest
    volumes:
      - ./prometheus.yml:/etc/prometheus/prometheus.yml
      - prometheus_data:/prometheus
    ports:
      - "9090:9090"
    restart: unless-stopped

  grafana:
    image: grafana/grafana:latest
    environment:
      GF_SECURITY_ADMIN_PASSWORD__FILE: /run/secrets/grafana_password
    volumes:
      - grafana_data:/var/lib/grafana
      - ./grafana/dashboards:/etc/grafana/provisioning/dashboards
    ports:
      - "3000:3000"
    secrets:
      - grafana_password
    restart: unless-stopped

volumes:
  postgres_data:
  prometheus_data:
  grafana_data:

secrets:
  db_password:
    file: ./secrets/db_password.txt
  grafana_password:
    file: ./secrets/grafana_password.txt
```

See [README-DOCKER.md](../README-DOCKER.md) for development setup and quick start.

### Kubernetes Deployment

**Deployment Manifest:**

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: flowplane-control-plane
  namespace: flowplane
spec:
  replicas: 2
  strategy:
    type: RollingUpdate
    rollingUpdate:
      maxUnavailable: 0
      maxSurge: 1
  selector:
    matchLabels:
      app: flowplane-cp
  template:
    metadata:
      labels:
        app: flowplane-cp
      annotations:
        prometheus.io/scrape: "true"
        prometheus.io/port: "8080"
        prometheus.io/path: "/metrics"
    spec:
      serviceAccountName: flowplane
      containers:
      - name: flowplane
        image: flowplane:v0.0.1
        ports:
        - name: http
          containerPort: 8080
          protocol: TCP
        - name: xds
          containerPort: 50051
          protocol: TCP
        env:
        - name: DATABASE_URL
          valueFrom:
            secretKeyRef:
              name: flowplane-secrets
              key: database-url
        - name: FLOWPLANE_API_BIND_ADDRESS
          value: "0.0.0.0"
        - name: FLOWPLANE_XDS_BIND_ADDRESS
          value: "0.0.0.0"
        - name: FLOWPLANE_ENABLE_METRICS
          value: "true"
        - name: RUST_LOG
          value: "info"
        volumeMounts:
        - name: tls-certs
          mountPath: /etc/flowplane/certs
          readOnly: true
        livenessProbe:
          httpGet:
            path: /healthz
            port: 8080
          initialDelaySeconds: 10
          periodSeconds: 10
        readinessProbe:
          httpGet:
            path: /healthz
            port: 8080
          initialDelaySeconds: 5
          periodSeconds: 5
        resources:
          requests:
            memory: "256Mi"
            cpu: "250m"
          limits:
            memory: "1Gi"
            cpu: "1000m"
      volumes:
      - name: tls-certs
        secret:
          secretName: flowplane-tls
---
apiVersion: v1
kind: Service
metadata:
  name: flowplane-api
  namespace: flowplane
spec:
  type: LoadBalancer
  ports:
  - name: http
    port: 8080
    targetPort: 8080
  selector:
    app: flowplane-cp
---
apiVersion: v1
kind: Service
metadata:
  name: flowplane-xds
  namespace: flowplane
spec:
  type: ClusterIP
  ports:
  - name: xds
    port: 50051
    targetPort: 50051
  selector:
    app: flowplane-cp
```

**ConfigMap for Envoy Proxies:**

```yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: envoy-bootstrap
  namespace: flowplane
data:
  envoy.yaml: |
    admin:
      address:
        socket_address:
          address: 0.0.0.0
          port_value: 9901

    node:
      cluster: envoy-cluster
      id: envoy-{POD_NAME}

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
                          address: flowplane-xds.flowplane.svc.cluster.local
                          port_value: 50051
          transport_socket:
            name: envoy.transport_sockets.tls
            typed_config:
              "@type": type.googleapis.com/envoy.extensions.transport_sockets.tls.v3.UpstreamTlsContext
              common_tls_context:
                tls_certificates:
                  - certificate_chain:
                      filename: /etc/envoy/certs/tls.crt
                    private_key:
                      filename: /etc/envoy/certs/tls.key
                validation_context:
                  trusted_ca:
                    filename: /etc/envoy/certs/ca.crt
```

## Monitoring & Observability

### Metrics

Flowplane exposes Prometheus metrics at `/metrics` endpoint.

**Prometheus Configuration:**

```yaml
# prometheus.yml
global:
  scrape_interval: 15s
  evaluation_interval: 15s

scrape_configs:
  - job_name: 'flowplane'
    static_configs:
      - targets: ['flowplane-cp:8080']
    metrics_path: '/metrics'
```

**Key Metrics to Monitor:**

| Metric | Description | Alert Threshold |
|--------|-------------|-----------------|
| `http_requests_total` | Total API requests | Rate > 1000/s |
| `http_request_duration_seconds` | Request latency | p99 > 1s |
| `flowplane_active_tokens` | Active tokens count | - |
| `flowplane_xds_connections` | Active xDS connections | Sudden drops |
| `flowplane_db_connection_pool_size` | DB pool usage | > 90% |
| `flowplane_audit_log_writes` | Audit events/sec | Spikes |

### Logging

**Structured JSON Logging:**

```bash
# Enable JSON logging
RUST_LOG=info,flowplane=debug

# Log output format
{
  "timestamp": "2025-10-07T22:30:00Z",
  "level": "INFO",
  "target": "flowplane::api",
  "fields": {
    "message": "API request processed",
    "method": "POST",
    "path": "/api/v1/clusters",
    "status": 201,
    "duration_ms": 45,
    "user_id": "fp_pat_abc123"
  }
}
```

**Log Aggregation:**

Use centralized logging (ELK, Loki, CloudWatch):

```yaml
# Fluentd configuration for Kubernetes
apiVersion: v1
kind: ConfigMap
metadata:
  name: fluentd-config
data:
  fluent.conf: |
    <source>
      @type tail
      path /var/log/containers/flowplane-*.log
      pos_file /var/log/flowplane.log.pos
      tag kubernetes.flowplane
      <parse>
        @type json
        time_key timestamp
        time_format %Y-%m-%dT%H:%M:%S%z
      </parse>
    </source>

    <match kubernetes.flowplane>
      @type elasticsearch
      host elasticsearch
      port 9200
      index_name flowplane
      type_name fluentd
    </match>
```

### Distributed Tracing

**Jaeger Integration:**

```bash
# Enable tracing
FLOWPLANE_ENABLE_TRACING=true
FLOWPLANE_JAEGER_ENDPOINT=http://jaeger:14268/api/traces
FLOWPLANE_SERVICE_NAME=flowplane-cp-prod

# Start Jaeger
docker run -d --name jaeger \
  -p 16686:16686 \
  -p 14268:14268 \
  jaegertracing/all-in-one:latest
```

**Trace Queries:**
- API request → database → xDS push → Envoy ACK
- Token validation → audit log write
- OpenAPI import → resource generation

### Audit Logs

Query audit logs directly from PostgreSQL:

```sql
-- Recent authentication failures
SELECT * FROM audit_logs
WHERE event_type = 'auth.failed'
AND created_at > NOW() - INTERVAL '1 hour'
ORDER BY created_at DESC;

-- Token operations by user
SELECT event_type, COUNT(*) as count
FROM audit_logs
WHERE user_id = 'fp_pat_abc123'
GROUP BY event_type;

-- Resource changes
SELECT * FROM audit_logs
WHERE event_type LIKE 'resource.%'
AND created_at > NOW() - INTERVAL '24 hours'
ORDER BY created_at DESC;
```

## Backup & Recovery

### Database Backups

**Automated PostgreSQL Backups:**

```bash
#!/bin/bash
# /etc/cron.daily/flowplane-backup.sh

BACKUP_DIR="/backups/flowplane"
DATE=$(date +%Y%m%d_%H%M%S)
BACKUP_FILE="flowplane_${DATE}.sql.gz"

# Create backup
pg_dump -h postgres -U flowplane flowplane | gzip > "$BACKUP_DIR/$BACKUP_FILE"

# Verify backup
gunzip -t "$BACKUP_DIR/$BACKUP_FILE"

# Rotate old backups (keep 30 days)
find "$BACKUP_DIR" -name "flowplane_*.sql.gz" -mtime +30 -delete

# Upload to S3
aws s3 cp "$BACKUP_DIR/$BACKUP_FILE" s3://backups/flowplane/
```

**Restore from Backup:**

```bash
# Stop control plane
docker-compose stop flowplane-cp

# Restore database
gunzip -c flowplane_20251007.sql.gz | psql -h postgres -U flowplane flowplane

# Restart control plane
docker-compose up -d flowplane-cp

# Verify
curl -H "Authorization: Bearer $TOKEN" http://flowplane:8080/api/v1/clusters
```

### Configuration Backup

```bash
# Backup all resources
curl -H "Authorization: Bearer $TOKEN" http://flowplane:8080/api/v1/clusters > clusters.json
curl -H "Authorization: Bearer $TOKEN" http://flowplane:8080/api/v1/routes > routes.json
curl -H "Authorization: Bearer $TOKEN" http://flowplane:8080/api/v1/listeners > listeners.json
curl -H "Authorization: Bearer $TOKEN" http://flowplane:8080/api/v1/tokens > tokens.json

# Restore resources
for file in clusters.json routes.json listeners.json; do
  jq -c '.[]' "$file" | while read -r resource; do
    curl -X POST http://flowplane:8080/api/v1/$(basename $file .json | sed 's/s$//') \
      -H "Authorization: Bearer $TOKEN" \
      -H "Content-Type: application/json" \
      -d "$resource"
  done
done
```

## Scaling & Performance

### Horizontal Scaling

**Control Plane Scaling:**

Flowplane is stateless (all state in PostgreSQL), allowing horizontal scaling:

```bash
# Docker Swarm
docker service scale flowplane-cp=3

# Kubernetes
kubectl scale deployment flowplane-control-plane --replicas=3
```

**Load Balancer Configuration:**

```nginx
# nginx.conf
upstream flowplane_api {
    least_conn;
    server flowplane-cp-1:8080 max_fails=3 fail_timeout=30s;
    server flowplane-cp-2:8080 max_fails=3 fail_timeout=30s;
    server flowplane-cp-3:8080 max_fails=3 fail_timeout=30s;
}

server {
    listen 443 ssl http2;
    server_name flowplane.example.com;

    ssl_certificate /etc/nginx/certs/flowplane.crt;
    ssl_certificate_key /etc/nginx/certs/flowplane.key;

    location / {
        proxy_pass http://flowplane_api;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }
}
```

### Database Performance

**PostgreSQL Tuning:**

```ini
# postgresql.conf
shared_buffers = 256MB
effective_cache_size = 1GB
maintenance_work_mem = 64MB
checkpoint_completion_target = 0.9
wal_buffers = 16MB
default_statistics_target = 100
random_page_cost = 1.1
effective_io_concurrency = 200
work_mem = 4MB
min_wal_size = 1GB
max_wal_size = 4GB
max_connections = 100
```

**Connection Pooling:**

Use PgBouncer for connection pooling:

```ini
# pgbouncer.ini
[databases]
flowplane = host=postgres port=5432 dbname=flowplane

[pgbouncer]
listen_port = 6432
listen_addr = *
auth_type = md5
auth_file = /etc/pgbouncer/userlist.txt
pool_mode = transaction
max_client_conn = 1000
default_pool_size = 25
```

Update DATABASE_URL:
```bash
DATABASE_URL=postgresql://flowplane:password@pgbouncer:6432/flowplane
```

### Resource Limits

**Recommended Limits:**

| Deployment Size | Control Plane | PostgreSQL | Envoy Proxies |
|----------------|---------------|------------|---------------|
| **Small** (< 100 Envoys) | 256MB / 0.5 CPU | 1GB / 1 CPU | 128MB / 0.25 CPU |
| **Medium** (100-500 Envoys) | 512MB / 1 CPU | 2GB / 2 CPU | 256MB / 0.5 CPU |
| **Large** (500-2000 Envoys) | 1GB / 2 CPU | 4GB / 4 CPU | 512MB / 1 CPU |
| **Extra Large** (2000+ Envoys) | 2GB / 4 CPU | 8GB / 8 CPU | 1GB / 2 CPU |

## Troubleshooting

### Common Issues

#### 1. Bootstrap Token Not Found

**Symptoms:**
- No token displayed on first startup
- "Token: fp_pat_..." line missing from logs

**Solutions:**
```bash
# Check full logs
docker logs flowplane-cp 2>&1 | grep -A 10 "BOOTSTRAP"

# If token was already generated (check audit logs)
docker exec -it postgres psql -U flowplane -c \
  "SELECT * FROM audit_logs WHERE event_type = 'auth.token.seeded' ORDER BY created_at DESC LIMIT 1;"

# Generate new admin token manually (requires database access)
# This is NOT recommended - better to start fresh
```

#### 2. Database Connection Failures

**Symptoms:**
```
Error: Failed to connect to database
Error: connection refused
```

**Solutions:**
```bash
# Verify PostgreSQL is running
docker ps | grep postgres

# Test connection
psql -h postgres -U flowplane -d flowplane

# Check DATABASE_URL format
echo $DATABASE_URL
# Should be: postgresql://user:pass@host:port/database

# Check network connectivity (Docker)
docker exec flowplane-cp ping postgres

# Check PostgreSQL logs
docker logs postgres
```

#### 3. xDS Connection Issues

**Symptoms:**
- Envoy reports "upstream connect error"
- Control plane logs show "TLS handshake failed"

**Solutions:**
```bash
# Verify xDS server is listening
netstat -tlnp | grep 50051

# Test xDS connection (plaintext)
grpcurl -plaintext flowplane:50051 list

# Verify mTLS certificates
openssl s_client -connect flowplane:50051 \
  -cert /path/to/client.crt \
  -key /path/to/client.key \
  -CAfile /path/to/ca.crt

# Check certificate expiry
openssl x509 -in /etc/flowplane/certs/xds-server.pem -noout -dates

# Envoy debug logging
RUST_LOG=debug,flowplane::xds_server=trace cargo run --bin flowplane
```

#### 4. High Memory Usage

**Symptoms:**
- Control plane OOM kills
- Memory usage grows continuously

**Solutions:**
```bash
# Check current memory usage
docker stats flowplane-cp

# Profile memory (requires debug build)
cargo flamegraph --bin flowplane

# Reduce connection pool size
# In code: sqlx::postgres::PgPoolOptions::new().max_connections(25)

# Check for connection leaks
docker exec -it postgres psql -U flowplane -c \
  "SELECT pid, usename, application_name, state, query FROM pg_stat_activity;"

# Increase memory limits
docker run --memory=1g --memory-swap=2g flowplane:v0.0.1
```

#### 5. Slow API Responses

**Symptoms:**
- API latency > 1s
- Timeout errors

**Solutions:**
```bash
# Enable query logging
RUST_LOG=sqlx=debug cargo run --bin flowplane

# Check database slow queries
docker exec -it postgres psql -U flowplane -c \
  "SELECT query, mean_exec_time, calls FROM pg_stat_statements ORDER BY mean_exec_time DESC LIMIT 10;"

# Add database indexes (if missing)
CREATE INDEX idx_clusters_name ON clusters(name);
CREATE INDEX idx_routes_name ON routes(name);
CREATE INDEX idx_listeners_name ON listeners(name);
CREATE INDEX idx_tokens_token_id ON tokens(token_id);
CREATE INDEX idx_audit_logs_created_at ON audit_logs(created_at);

# Check API request metrics
curl http://flowplane:8080/metrics | grep http_request_duration
```

### Health Checks

**Control Plane Health:**

```bash
# Basic health check
curl http://flowplane:8080/healthz
# Expected: 200 OK

# Metrics endpoint
curl http://flowplane:8080/metrics
# Expected: Prometheus metrics

# API endpoint test
curl -H "Authorization: Bearer $TOKEN" http://flowplane:8080/api/v1/clusters
# Expected: JSON array of clusters
```

**Database Health:**

```bash
# Connection test
psql -h postgres -U flowplane -d flowplane -c "SELECT 1;"

# Table check
psql -h postgres -U flowplane -d flowplane -c "\dt"

# Recent audit logs
psql -h postgres -U flowplane -d flowplane -c \
  "SELECT COUNT(*) FROM audit_logs WHERE created_at > NOW() - INTERVAL '1 hour';"
```

### Getting Help

**Debug Information to Collect:**

```bash
# Version info
curl http://flowplane:8080/api-docs/openapi.json | jq .info.version

# Logs (last 100 lines)
docker logs --tail 100 flowplane-cp

# Environment
docker exec flowplane-cp env | grep FLOWPLANE

# Resource usage
docker stats flowplane-cp --no-stream

# Database connection count
docker exec -it postgres psql -U flowplane -c \
  "SELECT count(*) FROM pg_stat_activity WHERE datname = 'flowplane';"

# Active xDS connections (from metrics)
curl http://flowplane:8080/metrics | grep flowplane_xds_connections
```

**Support Channels:**
- **GitHub Issues**: https://github.com/rajeevramani/flowplane/issues
- **Documentation**: https://github.com/rajeevramani/flowplane/tree/main/docs
- **Audit Logs**: Check `audit_logs` table for security events

## Additional Resources

- [README.md](../README.md) - Quick start and overview
- [README-DOCKER.md](../README-DOCKER.md) - Docker development setup
- [authentication.md](authentication.md) - Token management
- [tls.md](tls.md) - Certificate generation workflows
- [architecture.md](architecture.md) - System design
- [getting-started.md](getting-started.md) - Step-by-step tutorial
