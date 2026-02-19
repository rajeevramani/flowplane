# Multi-Region Deployment Guide

This guide covers deploying Flowplane across multiple geographic regions for high availability and low latency.

## Architecture Overview

Multi-region deployments require careful consideration of:
- **Database**: Shared PostgreSQL with replication
- **Control Plane**: Single or regional control planes
- **Dataplanes**: Regional Envoy instances
- **Network**: Cross-region connectivity

### Topology Options

#### Option 1: Centralized Control Plane (Recommended)

Single control plane with regional dataplanes:

```
                    ┌──────────────────────┐
                    │  Central Region      │
                    │  ┌────────────────┐  │
                    │  │  Flowplane CP  │  │
                    │  │  + PostgreSQL  │  │
                    │  └───────┬────────┘  │
                    └──────────┼───────────┘
                               │
          ┌────────────────────┼────────────────────┐
          │                    │                    │
          ▼                    ▼                    ▼
┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐
│  Region US-East │  │  Region EU-West │  │  Region AP-East │
│  ┌───────────┐  │  │  ┌───────────┐  │  │  ┌───────────┐  │
│  │  Envoy    │  │  │  │  Envoy    │  │  │  │  Envoy    │  │
│  │  Dataplane│  │  │  │  Dataplane│  │  │  │  Dataplane│  │
│  └───────────┘  │  │  └───────────┘  │  │  └───────────┘  │
└─────────────────┘  └─────────────────┘  └─────────────────┘
```

**Pros**: Simple, single source of truth, consistent configuration
**Cons**: xDS latency from remote regions, single point of failure

#### Option 2: Regional Control Planes with Shared Database

Control plane per region, shared PostgreSQL:

```
┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐
│  Region US-East │  │  Region EU-West │  │  Region AP-East │
│  ┌───────────┐  │  │  ┌───────────┐  │  │  ┌───────────┐  │
│  │Flowplane  │  │  │  │Flowplane  │  │  │  │Flowplane  │  │
│  │  CP       │  │  │  │  CP       │  │  │  │  CP       │  │
│  └─────┬─────┘  │  │  └─────┬─────┘  │  │  └─────┬─────┘  │
│        │        │  │        │        │  │        │        │
│  ┌───────────┐  │  │  ┌───────────┐  │  │  ┌───────────┐  │
│  │  Envoy    │  │  │  │  Envoy    │  │  │  │  Envoy    │  │
│  │  Dataplane│  │  │  │  Dataplane│  │  │  │  Dataplane│  │
│  └───────────┘  │  │  └───────────┘  │  │  └───────────┘  │
└────────┬────────┘  └────────┬────────┘  └────────┬────────┘
         │                    │                    │
         └────────────────────┼────────────────────┘
                              │
                    ┌─────────▼─────────┐
                    │    PostgreSQL     │
                    │  Primary + Read   │
                    │     Replicas      │
                    └───────────────────┘
```

**Pros**: Low xDS latency, regional failover
**Cons**: More complex, requires careful database synchronization

## Database Requirements

### PostgreSQL (Required for Multi-Region)

Multi-region deployments require a shared PostgreSQL instance accessible from all regions:

#### Primary-Replica Setup

```
Primary (us-east-1)
    ├── Sync Replica (us-east-2)      # HA within region
    ├── Async Replica (eu-west-1)     # Cross-region read
    └── Async Replica (ap-east-1)     # Cross-region read
```

#### Connection Configuration

```bash
# Primary (writes)
FLOWPLANE_DATABASE_URL=postgresql://flowplane:pass@primary.postgres.example.com:5432/flowplane?sslmode=require

# Read replica (optional, for read-heavy workloads)
FLOWPLANE_DATABASE_READ_URL=postgresql://flowplane:pass@replica.postgres.example.com:5432/flowplane?sslmode=require
```

#### Managed PostgreSQL Options

- **AWS RDS/Aurora**: Multi-AZ, read replicas, global database
- **GCP Cloud SQL**: Regional instances with read replicas
- **Azure Database for PostgreSQL**: Flexible server with read replicas
- **CockroachDB**: Distributed SQL with multi-region support

### Aurora Global Database Example

```yaml
# AWS Aurora Global Database
# Primary cluster in us-east-1
# Secondary clusters in eu-west-1 and ap-southeast-1

# Environment for US-East control plane
FLOWPLANE_DATABASE_URL=postgresql://admin:pass@flowplane-us.cluster-xxx.us-east-1.rds.amazonaws.com:5432/flowplane

# Environment for EU-West control plane (read replica endpoint)
FLOWPLANE_DATABASE_URL=postgresql://admin:pass@flowplane-eu.cluster-xxx.eu-west-1.rds.amazonaws.com:5432/flowplane
```

## xDS Server Distribution

### Single xDS Server (Centralized)

All dataplanes connect to the central control plane:

```yaml
# Envoy bootstrap for EU dataplane
node:
  cluster: eu-west-dataplane
  id: envoy-eu-001

dynamic_resources:
  ads_config:
    api_type: GRPC
    transport_api_version: V3
    grpc_services:
    - envoy_grpc:
        cluster_name: xds_cluster

static_resources:
  clusters:
  - name: xds_cluster
    type: STRICT_DNS
    lb_policy: ROUND_ROBIN
    load_assignment:
      cluster_name: xds_cluster
      endpoints:
      - lb_endpoints:
        - endpoint:
            address:
              socket_address:
                address: flowplane.us-east-1.internal  # Central CP
                port_value: 50051
```

### Regional xDS Servers

Each region has its own control plane:

```yaml
# Envoy bootstrap for EU dataplane
node:
  cluster: eu-west-dataplane
  id: envoy-eu-001

static_resources:
  clusters:
  - name: xds_cluster
    type: STRICT_DNS
    load_assignment:
      endpoints:
      - lb_endpoints:
        - endpoint:
            address:
              socket_address:
                address: flowplane.eu-west-1.internal  # Regional CP
                port_value: 50051
```

## Dataplane Configuration

### Regional Dataplane Setup

Create dataplanes for each region via API:

```bash
# US-East dataplane
curl -X POST http://flowplane.example.com/api/v1/teams/platform/dataplanes \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "team": "platform",
    "name": "us-east-production",
    "gateway_host": "envoy-us-east.example.com",
    "description": "Production dataplane in US-East region"
  }'

# EU-West dataplane
curl -X POST http://flowplane.example.com/api/v1/teams/platform/dataplanes \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "team": "platform",
    "name": "eu-west-production",
    "gateway_host": "envoy-eu-west.example.com",
    "description": "Production dataplane in EU-West region"
  }'

# AP-East dataplane
curl -X POST http://flowplane.example.com/api/v1/teams/platform/dataplanes \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "team": "platform",
    "name": "ap-east-production",
    "gateway_host": "envoy-ap-east.example.com",
    "description": "Production dataplane in AP-East region"
  }'
```

### Listener Assignment

Assign listeners to specific dataplanes:

```bash
# Create listener assigned to US-East dataplane
curl -X POST http://flowplane.example.com/api/v1/listeners \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "team": "platform",
    "name": "api-listener-us",
    "address": "0.0.0.0",
    "port": 10000,
    "dataplane_id": "dp_us_east_xxx"
  }'
```

## Failover Strategies

### Database Failover

#### Aurora Global Database Failover

```bash
# Promote secondary cluster to primary
aws rds failover-global-cluster \
  --global-cluster-identifier flowplane-global \
  --target-db-cluster-identifier flowplane-eu-cluster
```

#### Manual Failover

1. Stop writes to primary
2. Verify replica is caught up
3. Promote replica to primary
4. Update `FLOWPLANE_DATABASE_URL` for all control planes
5. Restart control planes

### Control Plane Failover

For regional control planes:

1. Update DNS to point to healthy control plane
2. Dataplanes reconnect automatically via xDS retry
3. No configuration loss (shared database)

### Dataplane Failover

Envoy handles upstream failover automatically with:
- Health checks
- Circuit breakers
- Outlier detection

## Network Considerations

### Cross-Region Latency

| Route | Typical Latency |
|-------|-----------------|
| US-East ↔ US-West | 60-80ms |
| US-East ↔ EU-West | 80-100ms |
| US-East ↔ AP-East | 150-200ms |

### VPN/Private Connectivity

For secure cross-region communication:
- AWS Transit Gateway
- GCP Cloud Interconnect
- Azure ExpressRoute

### mTLS for xDS

Enable mTLS for xDS connections:

```bash
# Control plane
FLOWPLANE_XDS_TLS_CERT_PATH=/etc/tls/xds-server.crt
FLOWPLANE_XDS_TLS_KEY_PATH=/etc/tls/xds-server.key
FLOWPLANE_XDS_TLS_CLIENT_CA_PATH=/etc/tls/ca.crt
FLOWPLANE_XDS_TLS_REQUIRE_CLIENT_CERT=true
```

## Deployment Checklist

- [ ] PostgreSQL with cross-region replication deployed
- [ ] Control plane(s) deployed with correct database URL
- [ ] Dataplanes registered with gateway_host
- [ ] Listeners assigned to appropriate dataplanes
- [ ] DNS configured for regional routing
- [ ] mTLS enabled for xDS communication
- [ ] Health checks and monitoring configured
- [ ] Failover procedures documented and tested

## Monitoring

### Key Metrics

- **Database replication lag**: Should be < 1s for async replicas
- **xDS connection count**: Number of connected dataplanes
- **Configuration sync latency**: Time for config changes to propagate
- **Cross-region latency**: RTT between regions

### Alerting

```yaml
# Prometheus alerting rules
groups:
- name: flowplane-multi-region
  rules:
  - alert: HighReplicationLag
    expr: pg_replication_lag_seconds > 5
    for: 5m
    labels:
      severity: warning
    annotations:
      summary: "High database replication lag"

  - alert: DataplaneDisconnected
    expr: flowplane_xds_connected_dataplanes < expected_count
    for: 2m
    labels:
      severity: critical
    annotations:
      summary: "Dataplane disconnected from control plane"
```

## Next Steps

- Configure [multi-dataplane architecture](multi-dataplane.md)
- Review [Kubernetes deployment](kubernetes.md)
- Set up [TLS configuration](../tls.md)
