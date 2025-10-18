# Flowplane Metrics Monitoring Guide

This guide explains how to visualize Flowplane's team-based metrics using Prometheus and Grafana.

## 📊 Information Flow Overview

```
┌─────────────────┐
│   Flowplane     │  1. Exposes /metrics endpoint
│ Control Plane   │     on port 9090
│                 │
│ ┌─────────────┐ │
│ │  Metrics    │ │
│ │  Exporter   │ │
│ │ (port 9090) │ │
│ └──────┬──────┘ │
└────────┼────────┘
         │
         │ HTTP GET /metrics
         │ (every 10-15 seconds)
         │
         ▼
┌────────┴────────┐
│   Prometheus    │  2. Scrapes metrics periodically
│                 │     Stores time-series data
│  Time-Series DB │
│  (port 9091)    │
└────────┬────────┘
         │
         │ PromQL queries
         │
         ▼
┌────────┴────────┐
│    Grafana      │  3. Visualizes metrics
│                 │     Creates dashboards
│  (port 3000)    │
│                 │
│  Dashboard UI   │
└─────────────────┘
         │
         │
         ▼
    👤 User
```

## 🔄 Detailed Information Flow

### Step 1: Flowplane Exposes Metrics

When Flowplane starts, it:
1. Initializes the Prometheus metrics exporter (`src/observability/metrics.rs:init_metrics()`)
2. Starts an HTTP server on port 9090 (configurable via `FLOWPLANE_METRICS_PORT`)
3. Exposes the `/metrics` endpoint in Prometheus text format

**Example metrics output:**
```prometheus
# HELP xds_team_resources_total Number of resources served per team
# TYPE xds_team_resources_total gauge
xds_team_resources_total{resource_type="cluster",team="payments"} 42
xds_team_resources_total{resource_type="route",team="payments"} 15

# HELP auth_cross_team_access_attempts_total Cross-team access attempts
# TYPE auth_cross_team_access_attempts_total counter
auth_cross_team_access_attempts_total{from_team="payments",to_team="billing",resource_type="clusters"} 5
```

### Step 2: Prometheus Scrapes Metrics

Prometheus:
1. Reads its configuration (`prometheus.yml`)
2. Every 10-15 seconds, sends HTTP GET request to `http://host.docker.internal:9090/metrics`
3. Parses the Prometheus text format
4. Stores the metrics in its time-series database (TSDB)
5. Keeps historical data for querying

**Configuration:** `monitoring/prometheus/prometheus.yml`
```yaml
scrape_configs:
  - job_name: 'flowplane'
    static_configs:
      - targets: ['host.docker.internal:9090']
    scrape_interval: 10s
```

### Step 3: Grafana Visualizes Metrics

Grafana:
1. Connects to Prometheus as a data source
2. Uses PromQL (Prometheus Query Language) to query metrics
3. Renders visualizations (time series, gauges, pie charts, etc.)
4. Refreshes dashboards automatically (every 10 seconds by default)

**Example PromQL queries:**
```promql
# Total resources per team
sum by (team) (xds_team_resources_total)

# Cross-team access rate
rate(auth_cross_team_access_attempts_total[5m]) * 60

# Active connections
xds_team_connections
```

## 🚀 Quick Start

### 1. Start Flowplane

```bash
# Make sure Flowplane is running with metrics enabled
cargo run --bin flowplane
```

Verify metrics are exposed:
```bash
curl http://localhost:9090/metrics | grep xds_team
```

### 2. Start Prometheus and Grafana

```bash
# Start monitoring stack
docker-compose -f docker-compose-monitoring.yml up -d

# Check status
docker-compose -f docker-compose-monitoring.yml ps
```

### 3. Access Dashboards

Open your browser:

- **Grafana Dashboard**: http://localhost:3000
  - Username: `admin`
  - Password: `admin`
  - Navigate to: Dashboards → Flowplane → Team-Based Metrics

- **Prometheus UI**: http://localhost:9091
  - Query metrics directly using PromQL

### 4. Verify Data Flow

```bash
# 1. Check Flowplane is exposing metrics
curl http://localhost:9090/metrics | head -20

# 2. Check Prometheus is scraping
# Open http://localhost:9091/targets
# Should show "flowplane" target as UP

# 3. Query Prometheus
curl 'http://localhost:9091/api/v1/query?query=xds_team_resources_total'

# 4. View in Grafana
# Open http://localhost:3000 and go to the dashboard
```

## 📈 Available Dashboards

### Flowplane - Team-Based Metrics

Pre-configured dashboard showing:

1. **Team Resource Distribution** (Time Series)
   - Clusters, routes, and listeners per team over time
   - Legend shows current and max values

2. **Active xDS Connections per Team** (Time Series)
   - Real-time connection tracking
   - Helps identify connection patterns

3. **Cross-Team Access Attempts** (Bar Gauge)
   - Security monitoring
   - Shows unauthorized access attempts per minute
   - Color-coded: green (0-10), yellow (10-50), red (50+)

4. **Resource Distribution by Team** (Pie Chart)
   - Overall resource allocation across teams
   - Percentage breakdown

5. **Summary Stats**:
   - Total Teams
   - Total Resources
   - Cross-Team Access Attempts (Last Hour)
   - Total Active Connections

## 🔧 Configuration

### Adjust Scrape Interval

Edit `monitoring/prometheus/prometheus.yml`:
```yaml
scrape_configs:
  - job_name: 'flowplane'
    scrape_interval: 5s  # Scrape every 5 seconds (more frequent)
```

Then restart Prometheus:
```bash
docker-compose -f docker-compose-monitoring.yml restart prometheus
```

### Change Metrics Port in Flowplane

```bash
# Via environment variable
export FLOWPLANE_METRICS_PORT=9999
cargo run --bin flowplane

# Or in config.toml
[observability]
metrics_port = 9999
```

Don't forget to update `prometheus.yml` to match the new port.

### Add Custom Metrics

To add your own metrics:

1. **Define metric in `src/observability/metrics.rs`:**
```rust
pub fn record_custom_metric(&self, value: u64) {
    counter!("custom_metric_total").increment(value);
}
```

2. **Register in `register_team_metrics()`:**
```rust
describe_counter!(
    "custom_metric_total",
    Unit::Count,
    "Description of custom metric"
);
```

3. **Emit metric in your code:**
```rust
crate::observability::metrics::record_custom_metric(42).await;
```

4. **Query in Grafana:**
```promql
custom_metric_total
```

## 🐛 Troubleshooting

### Prometheus shows target as DOWN

**Check 1:** Is Flowplane running?
```bash
curl http://localhost:9090/metrics
```

**Check 2:** Docker network connectivity
```bash
# On Linux, add to docker-compose-monitoring.yml:
services:
  prometheus:
    extra_hosts:
      - "host.docker.internal:host-gateway"
```

**Check 3:** Firewall blocking port 9090
```bash
# Allow port 9090 in firewall
sudo ufw allow 9090
```

### No data in Grafana

**Check 1:** Prometheus has data
```bash
# Query Prometheus directly
curl 'http://localhost:9091/api/v1/query?query=xds_team_resources_total'
```

**Check 2:** Grafana datasource configured
- Go to Configuration → Data Sources
- Should see "Prometheus" pointing to `http://prometheus:9090`

**Check 3:** Time range
- In Grafana, check the time picker (top right)
- Set to "Last 15 minutes" or "Last 1 hour"

### Metrics show zero values

This is normal if:
- No teams have been created yet
- No xDS clients are connected
- No resources have been created

**Generate test data:**
```bash
# Create a team-scoped resource via API
curl -X POST http://localhost:8080/api/v1/clusters \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer YOUR_TOKEN" \
  -d '{
    "name": "test-cluster",
    "endpoints": [{"host": "localhost", "port": 8080}]
  }'
```

## 📚 Learn More

### Prometheus Resources
- [Prometheus Documentation](https://prometheus.io/docs/)
- [PromQL Basics](https://prometheus.io/docs/prometheus/latest/querying/basics/)
- [Metric Types](https://prometheus.io/docs/concepts/metric_types/)

### Grafana Resources
- [Grafana Documentation](https://grafana.com/docs/)
- [Dashboard Best Practices](https://grafana.com/docs/grafana/latest/dashboards/build-dashboards/best-practices/)
- [PromQL in Grafana](https://grafana.com/docs/grafana/latest/datasources/prometheus/)

### Flowplane Metrics
- **Metric Types Used:**
  - **Gauge**: Values that can go up or down (resource counts, connections)
  - **Counter**: Values that only increase (access attempts)

- **Labels**: Key-value pairs for filtering
  - `team`: Team name
  - `resource_type`: cluster, route, listener
  - `from_team`, `to_team`: For access attempts

## 🧹 Cleanup

Stop and remove all containers:
```bash
docker-compose -f docker-compose-monitoring.yml down

# Remove volumes (deletes historical data)
docker-compose -f docker-compose-monitoring.yml down -v
```

## 🎯 Next Steps

1. **Explore Metrics**: Use Prometheus UI to run PromQL queries
2. **Customize Dashboard**: Edit the Grafana dashboard to add your own panels
3. **Set Alerts**: Configure Prometheus alerting rules
4. **Export Dashboards**: Save dashboard JSON for version control
5. **Add More Metrics**: Instrument additional parts of Flowplane

---

**Questions?** Check the Flowplane documentation or open an issue on GitHub.
