# Kubernetes Deployment Guide

This guide covers deploying Flowplane to Kubernetes for production use.

## Prerequisites

- Kubernetes cluster (1.25+)
- kubectl configured
- Helm 3.x (optional but recommended)
- PostgreSQL database (for production)

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                     Kubernetes Cluster                          │
├─────────────────────────────────────────────────────────────────┤
│  ┌──────────────────┐    ┌──────────────────┐                  │
│  │  Flowplane CP    │    │  PostgreSQL      │                  │
│  │  (Deployment)    │───▶│  (StatefulSet)   │                  │
│  │  Replicas: 1-3   │    │                  │                  │
│  └────────┬─────────┘    └──────────────────┘                  │
│           │                                                     │
│           ▼                                                     │
│  ┌──────────────────┐                                          │
│  │  Envoy Dataplane │                                          │
│  │  (DaemonSet or   │                                          │
│  │   Deployment)    │                                          │
│  └──────────────────┘                                          │
│                                                                 │
│  ┌──────────────────┐    ┌──────────────────┐                  │
│  │  Service: API    │    │  Service: xDS    │                  │
│  │  (LoadBalancer)  │    │  (ClusterIP)     │                  │
│  └──────────────────┘    └──────────────────┘                  │
└─────────────────────────────────────────────────────────────────┘
```

## Quick Start with Raw Manifests

### Namespace

```yaml
# namespace.yaml
apiVersion: v1
kind: Namespace
metadata:
  name: flowplane
```

### ConfigMap

```yaml
# configmap.yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: flowplane-config
  namespace: flowplane
data:
  FLOWPLANE_API_PORT: "8080"
  FLOWPLANE_API_BIND_ADDRESS: "0.0.0.0"
  FLOWPLANE_XDS_PORT: "50051"
  FLOWPLANE_XDS_BIND_ADDRESS: "0.0.0.0"
  FLOWPLANE_DATABASE_AUTO_MIGRATE: "true"
  FLOWPLANE_ENABLE_METRICS: "true"
  FLOWPLANE_METRICS_PORT: "9090"
  FLOWPLANE_LOG_LEVEL: "info"
  FLOWPLANE_JSON_LOGGING: "true"
```

### Secret

```yaml
# secret.yaml
apiVersion: v1
kind: Secret
metadata:
  name: flowplane-secrets
  namespace: flowplane
type: Opaque
stringData:
  FLOWPLANE_DATABASE_URL: "postgresql://flowplane:changeme@postgres:5432/flowplane?sslmode=require"
  FLOWPLANE_JWT_SECRET: "your-32-character-minimum-secret-key-here"
```

### Deployment

```yaml
# deployment.yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: flowplane
  namespace: flowplane
spec:
  replicas: 1
  selector:
    matchLabels:
      app: flowplane
  template:
    metadata:
      labels:
        app: flowplane
      annotations:
        prometheus.io/scrape: "true"
        prometheus.io/port: "9090"
    spec:
      serviceAccountName: flowplane
      containers:
      - name: flowplane
        image: ghcr.io/flowplane/flowplane:latest
        ports:
        - name: http
          containerPort: 8080
        - name: xds
          containerPort: 50051
        - name: metrics
          containerPort: 9090
        envFrom:
        - configMapRef:
            name: flowplane-config
        - secretRef:
            name: flowplane-secrets
        resources:
          requests:
            memory: "256Mi"
            cpu: "100m"
          limits:
            memory: "1Gi"
            cpu: "1000m"
        livenessProbe:
          httpGet:
            path: /health
            port: http
          initialDelaySeconds: 10
          periodSeconds: 30
        readinessProbe:
          httpGet:
            path: /health
            port: http
          initialDelaySeconds: 5
          periodSeconds: 10
        volumeMounts:
        - name: data
          mountPath: /app/data
      volumes:
      - name: data
        persistentVolumeClaim:
          claimName: flowplane-data
```

### Services

```yaml
# services.yaml
apiVersion: v1
kind: Service
metadata:
  name: flowplane-api
  namespace: flowplane
spec:
  type: LoadBalancer
  ports:
  - port: 80
    targetPort: 8080
    name: http
  selector:
    app: flowplane
---
apiVersion: v1
kind: Service
metadata:
  name: flowplane-xds
  namespace: flowplane
spec:
  type: ClusterIP
  ports:
  - port: 50051
    targetPort: 50051
    name: grpc
  selector:
    app: flowplane
```

### PersistentVolumeClaim

```yaml
# pvc.yaml
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: flowplane-data
  namespace: flowplane
spec:
  accessModes:
    - ReadWriteOnce
  resources:
    requests:
      storage: 10Gi
```

## PostgreSQL StatefulSet

For production, use PostgreSQL instead of SQLite:

```yaml
# postgres.yaml
apiVersion: apps/v1
kind: StatefulSet
metadata:
  name: postgres
  namespace: flowplane
spec:
  serviceName: postgres
  replicas: 1
  selector:
    matchLabels:
      app: postgres
  template:
    metadata:
      labels:
        app: postgres
    spec:
      containers:
      - name: postgres
        image: postgres:15
        ports:
        - containerPort: 5432
        env:
        - name: POSTGRES_DB
          value: flowplane
        - name: POSTGRES_USER
          value: flowplane
        - name: POSTGRES_PASSWORD
          valueFrom:
            secretKeyRef:
              name: postgres-secret
              key: password
        volumeMounts:
        - name: data
          mountPath: /var/lib/postgresql/data
  volumeClaimTemplates:
  - metadata:
      name: data
    spec:
      accessModes: ["ReadWriteOnce"]
      resources:
        requests:
          storage: 20Gi
---
apiVersion: v1
kind: Service
metadata:
  name: postgres
  namespace: flowplane
spec:
  ports:
  - port: 5432
  selector:
    app: postgres
```

## Envoy Dataplane DaemonSet

Deploy Envoy as a DaemonSet to run on every node:

```yaml
# envoy-daemonset.yaml
apiVersion: apps/v1
kind: DaemonSet
metadata:
  name: envoy
  namespace: flowplane
spec:
  selector:
    matchLabels:
      app: envoy
  template:
    metadata:
      labels:
        app: envoy
    spec:
      containers:
      - name: envoy
        image: envoyproxy/envoy:v1.31-latest
        args:
        - -c
        - /etc/envoy/bootstrap.yaml
        ports:
        - containerPort: 10000
          hostPort: 10000
        - containerPort: 9901
        volumeMounts:
        - name: config
          mountPath: /etc/envoy
      volumes:
      - name: config
        configMap:
          name: envoy-bootstrap
```

## Configuration Reference

### Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `FLOWPLANE_DATABASE_URL` | Database connection string | `sqlite://./data/flowplane.db` |
| `FLOWPLANE_DATABASE_MAX_CONNECTIONS` | Max pool connections | 10 |
| `FLOWPLANE_DATABASE_MIN_CONNECTIONS` | Min pool connections | 0 |
| `FLOWPLANE_API_PORT` | REST API port | 8080 |
| `FLOWPLANE_API_BIND_ADDRESS` | API bind address | 127.0.0.1 |
| `FLOWPLANE_XDS_PORT` | xDS gRPC port | 18000 |
| `FLOWPLANE_XDS_BIND_ADDRESS` | xDS bind address | 0.0.0.0 |
| `FLOWPLANE_ENABLE_METRICS` | Enable Prometheus metrics | true |
| `FLOWPLANE_METRICS_PORT` | Metrics port | 9090 |
| `FLOWPLANE_LOG_LEVEL` | Log level | info |
| `FLOWPLANE_JSON_LOGGING` | JSON log format | false |
| `FLOWPLANE_JWT_SECRET` | JWT signing secret | (required) |

### Ports

| Port | Protocol | Purpose |
|------|----------|---------|
| 8080 | HTTP | REST API and UI |
| 50051 | gRPC | xDS discovery service |
| 9090 | HTTP | Prometheus metrics |
| 50052 | gRPC | Access Log Service |
| 50053 | gRPC | External Processor |

## High Availability

For high availability, run multiple replicas with shared PostgreSQL:

```yaml
spec:
  replicas: 3
```

Note: Only one replica processes xDS at a time. Use leader election for production HA.

## Monitoring

### ServiceMonitor (Prometheus Operator)

```yaml
apiVersion: monitoring.coreos.com/v1
kind: ServiceMonitor
metadata:
  name: flowplane
  namespace: flowplane
spec:
  selector:
    matchLabels:
      app: flowplane
  endpoints:
  - port: metrics
    interval: 30s
```

## TLS Configuration

### xDS mTLS

```yaml
# In secret.yaml
stringData:
  FLOWPLANE_XDS_TLS_CERT_PATH: "/etc/tls/server.crt"
  FLOWPLANE_XDS_TLS_KEY_PATH: "/etc/tls/server.key"
  FLOWPLANE_XDS_TLS_CLIENT_CA_PATH: "/etc/tls/ca.crt"
  FLOWPLANE_XDS_TLS_REQUIRE_CLIENT_CERT: "true"
```

Mount certificates:
```yaml
volumeMounts:
- name: tls
  mountPath: /etc/tls
  readOnly: true
volumes:
- name: tls
  secret:
    secretName: flowplane-tls
```

## Troubleshooting

### Check pod status
```bash
kubectl get pods -n flowplane
kubectl logs -n flowplane deployment/flowplane
```

### Check database connectivity
```bash
kubectl exec -it -n flowplane deployment/flowplane -- \
  curl -s http://localhost:8080/health
```

### Check xDS connectivity
```bash
kubectl exec -it -n flowplane pod/envoy-xxxxx -- \
  curl -s http://localhost:9901/clusters
```

## Next Steps

- Configure [multi-dataplane deployment](multi-dataplane.md)
- Set up [multi-region deployment](multi-region.md)
- Review [TLS configuration](../tls.md)
