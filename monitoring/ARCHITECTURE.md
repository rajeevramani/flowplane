# Flowplane Metrics Architecture

## System Overview

```
┌─────────────────────────────────────────────────────────────────────┐
│                        Flowplane Control Plane                      │
│                                                                     │
│  ┌──────────────┐    ┌──────────────┐    ┌──────────────┐        │
│  │   API        │    │   xDS        │    │   Auth       │        │
│  │  Handlers    │    │  Service     │    │  Middleware  │        │
│  └──────┬───────┘    └──────┬───────┘    └──────┬───────┘        │
│         │                   │                    │                 │
│         │ emit metrics      │ emit metrics       │ emit metrics    │
│         └───────────────────┴────────────────────┘                 │
│                             │                                       │
│                             ▼                                       │
│                  ┌──────────────────────┐                          │
│                  │  MetricsRecorder     │                          │
│                  │  (metrics.rs)        │                          │
│                  └──────────┬───────────┘                          │
│                             │                                       │
│                             ▼                                       │
│                  ┌──────────────────────┐                          │
│                  │ Prometheus Exporter  │                          │
│                  │ (port 9090)          │                          │
│                  │ /metrics endpoint    │                          │
│                  └──────────┬───────────┘                          │
└─────────────────────────────┼───────────────────────────────────────┘
                              │
                              │ HTTP GET /metrics
                              │ (scrape every 10s)
                              │
                              ▼
          ┌───────────────────────────────────────┐
          │         Prometheus                    │
          │   Time-Series Database                │
          │                                       │
          │  • Scrapes metrics endpoint          │
          │  • Stores historical data            │
          │  • Provides PromQL query interface   │
          │  • Retention: 15 days (default)      │
          │                                       │
          │  Port: 9091 (mapped from 9090)       │
          └───────────────┬───────────────────────┘
                          │
                          │ PromQL queries
                          │
                          ▼
          ┌───────────────────────────────────────┐
          │           Grafana                     │
          │   Visualization Platform              │
          │                                       │
          │  • Connects to Prometheus            │
          │  • Executes PromQL queries           │
          │  • Renders dashboards                │
          │  • Auto-refresh every 10s            │
          │                                       │
          │  Port: 3000                           │
          └───────────────────────────────────────┘
```

## Metrics Flow Detail

### 1. Metric Emission (Flowplane → Prometheus Exporter)

```rust
// src/xds/services/database.rs
pub async fn list_clusters_by_team(&self, team: &str) -> Result<Vec<ClusterData>> {
    // Execute database query
    let clusters = self.repository.list_by_team(team).await?;

    // Emit metric
    crate::observability::metrics::update_team_resource_count(
        "cluster",
        team,
        clusters.len()
    ).await;

    Ok(clusters)
}
```

**Flow:**
1. Code calls `update_team_resource_count()`
2. Function acquires global metrics recorder
3. Calls `gauge!("xds_team_resources_total", &labels).set(count)`
4. Metrics crate stores value in memory
5. Value available at `/metrics` endpoint

### 2. Metric Scraping (Prometheus Exporter → Prometheus)

**Prometheus makes HTTP request:**
```http
GET http://host.docker.internal:9090/metrics HTTP/1.1
Host: host.docker.internal:9090
User-Agent: Prometheus/2.48.0
Accept: application/openmetrics-text
```

**Response (Prometheus text format):**
```prometheus
# HELP xds_team_resources_total Number of resources served per team
# TYPE xds_team_resources_total gauge
xds_team_resources_total{resource_type="cluster",team="payments"} 42
xds_team_resources_total{resource_type="route",team="payments"} 15
xds_team_resources_total{resource_type="listener",team="billing"} 3
```

**Prometheus processing:**
1. Parses text format
2. Extracts metric name, labels, and value
3. Creates time-series entry: `{timestamp, value}`
4. Stores in TSDB (compressed, indexed)

### 3. Metric Querying (Grafana → Prometheus)

**Grafana sends PromQL query:**
```http
POST http://prometheus:9090/api/v1/query_range HTTP/1.1
Content-Type: application/x-www-form-urlencoded

query=xds_team_resources_total{resource_type="cluster"}
start=1234567890
end=1234567900
step=15
```

**Prometheus response (JSON):**
```json
{
  "status": "success",
  "data": {
    "resultType": "matrix",
    "result": [
      {
        "metric": {
          "resource_type": "cluster",
          "team": "payments"
        },
        "values": [
          [1234567890, "42"],
          [1234567905, "42"],
          [1234567920, "43"]
        ]
      }
    ]
  }
}
```

**Grafana visualization:**
1. Parses JSON response
2. Extracts time-series data
3. Renders chart (line, bar, gauge, etc.)
4. Updates display every 10 seconds

## Metric Types in Flowplane

### Gauge Metrics

**Characteristics:**
- Can go up or down
- Represents current state
- Example: Resource counts, active connections

**Implementation:**
```rust
pub fn update_team_resource_count(&self, resource_type: &str, team: &str, count: usize) {
    let labels = [
        ("resource_type", resource_type.to_string()),
        ("team", team.to_string())
    ];
    gauge!("xds_team_resources_total", &labels).set(count as f64);
}
```

**Usage in PromQL:**
```promql
# Current value
xds_team_resources_total{team="payments"}

# Sum across all teams
sum(xds_team_resources_total)

# Average over time
avg_over_time(xds_team_resources_total[5m])
```

### Counter Metrics

**Characteristics:**
- Only increases (monotonic)
- Resets to 0 on restart
- Example: Access attempts, requests processed

**Implementation:**
```rust
pub fn record_cross_team_access_attempt(
    &self,
    from_team: &str,
    to_team: &str,
    resource_type: &str,
) {
    let labels = [
        ("from_team", from_team.to_string()),
        ("to_team", to_team.to_string()),
        ("resource_type", resource_type.to_string()),
    ];
    counter!("auth_cross_team_access_attempts_total", &labels).increment(1);
}
```

**Usage in PromQL:**
```promql
# Total count (resets on restart)
auth_cross_team_access_attempts_total

# Rate per second
rate(auth_cross_team_access_attempts_total[5m])

# Rate per minute
rate(auth_cross_team_access_attempts_total[5m]) * 60

# Total in last hour
increase(auth_cross_team_access_attempts_total[1h])
```

## Label Strategy

### Why Labels Matter

Labels enable multi-dimensional filtering and aggregation:

```promql
# All resources for payments team
xds_team_resources_total{team="payments"}

# Only clusters for payments team
xds_team_resources_total{team="payments", resource_type="cluster"}

# Sum all resources across teams
sum(xds_team_resources_total)

# Sum by team
sum by (team) (xds_team_resources_total)

# Sum by resource type
sum by (resource_type) (xds_team_resources_total)
```

### Label Cardinality

**Current labels:**
- `team`: ~10-100 unique values (low cardinality)
- `resource_type`: 3 values (cluster, route, listener) (very low)
- `from_team`, `to_team`: ~10-100 each (low)

**Impact:**
- Total time-series: ~300-10,000
- Memory usage: ~1-10 MB
- Query performance: Excellent

**Bad practice (avoid):**
```rust
// ❌ DON'T: High cardinality labels
gauge!("resource_metric", "resource_id" => resource_id)  // Thousands of IDs

// ✅ DO: Use aggregation
gauge!("resource_count", "team" => team)  // Tens of teams
```

## Performance Considerations

### Memory Usage

**Flowplane metrics exporter:**
- In-memory registry: ~1-5 MB
- Per metric: ~100 bytes
- Total: ~1,000 metrics × 100 bytes = ~100 KB

**Prometheus TSDB:**
- Per sample: ~1-2 bytes (compressed)
- Scrape interval: 10s
- Retention: 15 days
- Total samples: (1000 metrics) × (6 samples/min) × (60 min) × (24 hrs) × (15 days) = ~130M samples
- Storage: ~130-260 MB

### Network Traffic

**Per scrape:**
- Metrics endpoint response: ~10-50 KB
- Frequency: Every 10 seconds
- Daily traffic: (10 KB) × (6 scrapes/min) × (60 min) × (24 hrs) = ~86 MB/day

### Query Performance

**Fast queries (< 100ms):**
```promql
# Single metric, recent time
xds_team_resources_total{team="payments"}[5m]
```

**Medium queries (100ms - 1s):**
```promql
# Aggregation across labels
sum by (team) (xds_team_resources_total)
```

**Slow queries (> 1s):**
```promql
# Long time range with high resolution
xds_team_resources_total[30d:10s]
```

## Security Considerations

### Metrics Endpoint

**Current state:**
- Unprotected HTTP endpoint
- No authentication required
- Exposed on localhost by default

**Production recommendations:**
1. **Bind to localhost only:**
   ```rust
   format!("127.0.0.1:{}", self.metrics_port)
   ```

2. **Add authentication:**
   - Use reverse proxy (nginx) with basic auth
   - Or mTLS for Prometheus scraping

3. **Firewall rules:**
   - Restrict port 9090 to Prometheus server IP only

### Sensitive Data in Metrics

**Safe (current implementation):**
```prometheus
xds_team_resources_total{team="payments"} 42
auth_cross_team_access_attempts_total{from_team="payments",to_team="billing"} 5
```

**Unsafe (avoid):**
```prometheus
# ❌ DON'T expose PII
user_login{email="user@example.com"} 1

# ❌ DON'T expose secrets
api_key_usage{key="sk-abc123..."} 1
```

**Best practice:**
- Use IDs instead of names when possible
- Hash sensitive identifiers
- Aggregate before exposing

## Scaling Considerations

### Horizontal Scaling (Multiple Flowplane Instances)

**Challenge:** Each instance exposes its own metrics

**Solution 1: Federation (Simple)**
```yaml
# Prometheus config
scrape_configs:
  - job_name: 'flowplane'
    static_configs:
      - targets:
        - 'flowplane-1:9090'
        - 'flowplane-2:9090'
        - 'flowplane-3:9090'
```

**Solution 2: Pushgateway (Advanced)**
- Instances push metrics to central Pushgateway
- Prometheus scrapes Pushgateway
- Useful for short-lived jobs

### Prometheus Scaling

**When to scale:**
- > 1M active time-series
- > 10K samples/second ingestion
- > 100 GB storage

**Options:**
1. **Vertical scaling:** More RAM, faster SSD
2. **Horizontal scaling:** Prometheus federation
3. **Thanos/Cortex:** Distributed Prometheus

### Grafana Scaling

**Built-in features:**
- Multi-tenancy (separate orgs)
- LDAP/OAuth authentication
- Caching for dashboards
- Read replicas for Prometheus

## Troubleshooting Checklist

### No metrics in Grafana

```bash
# 1. Check Flowplane metrics endpoint
curl http://localhost:9090/metrics

# 2. Check Prometheus scraping
curl http://localhost:9091/api/v1/targets | jq

# 3. Check Prometheus data
curl 'http://localhost:9091/api/v1/query?query=up{job="flowplane"}' | jq

# 4. Check Grafana datasource
curl http://localhost:3000/api/datasources
```

### Metrics show zero

```bash
# Check if metrics are being emitted
curl http://localhost:9090/metrics | grep xds_team_resources_total

# If empty, emit some metrics by:
# 1. Creating resources via API
# 2. Connecting xDS clients
# 3. Triggering cross-team access
```

### High memory usage

```bash
# Check metric cardinality
curl http://localhost:9090/metrics | grep -c "^xds_team_resources_total"

# Check Prometheus TSDB stats
curl http://localhost:9091/api/v1/status/tsdb | jq
```

---

**Next:** See [README.md](README.md) for setup instructions and usage guide.
