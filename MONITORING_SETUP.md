# Flowplane Monitoring Setup - Quick Reference

## 📊 What You Just Got

A complete monitoring stack for Flowplane's team-based metrics with:
- **Prometheus**: Collects and stores metrics
- **Grafana**: Beautiful dashboards for visualization
- **Pre-configured dashboard**: Team-based metrics ready to use

## 🚀 Quick Start (3 Steps)

### Step 1: Start Flowplane
```bash
cargo run --bin flowplane
```

### Step 2: Start Monitoring Stack
```bash
./monitoring/start-monitoring.sh
```

### Step 3: View Dashboard
Open http://localhost:3000 in your browser
- Username: `admin`
- Password: `admin`
- Navigate to: **Dashboards → Flowplane → Team-Based Metrics**

## 📍 Access URLs

| Service | URL | Purpose |
|---------|-----|---------|
| **Grafana Dashboard** | http://localhost:3000 | Visualizations |
| **Prometheus UI** | http://localhost:9091 | Query metrics |
| **Flowplane Metrics** | http://localhost:9090/metrics | Raw metrics endpoint |

## 📈 What You'll See

The dashboard shows:
1. **Resource Distribution**: How many clusters/routes/listeners per team
2. **Active Connections**: xDS connections by team
3. **Security Events**: Cross-team access attempts
4. **Summary Stats**: Total teams, resources, connections

## 🔄 How It Works

```
Flowplane (port 9090)
    ↓ exposes /metrics
Prometheus (port 9091)
    ↓ scrapes every 10s
    ↓ stores time-series data
Grafana (port 3000)
    ↓ queries with PromQL
    ↓ renders dashboards
You (browser)
```

## 📚 Documentation

- **[monitoring/README.md](monitoring/README.md)** - Complete setup guide, troubleshooting
- **[monitoring/ARCHITECTURE.md](monitoring/ARCHITECTURE.md)** - Technical details, scaling, security

## 🛑 Stop Monitoring

```bash
docker-compose -f docker-compose-monitoring.yml down
```

## 💡 Example Queries

Try these in Prometheus UI (http://localhost:9091):

```promql
# Total resources per team
sum by (team) (xds_team_resources_total)

# Cross-team access rate (per minute)
rate(auth_cross_team_access_attempts_total[5m]) * 60

# Active connections
xds_team_connections
```

## 🐛 Troubleshooting

### No data showing?

1. **Check Flowplane is running:**
   ```bash
   curl http://localhost:9090/metrics
   ```

2. **Check Prometheus is scraping:**
   Open http://localhost:9091/targets
   Should show "flowplane" as **UP** (green)

3. **Generate test data:**
   Create some resources via the Flowplane API to see metrics update

### Prometheus shows target as DOWN?

On **Linux**, ensure Docker can reach your host:
```bash
# Already configured in docker-compose-monitoring.yml
# with: extra_hosts: - "host.docker.internal:host-gateway"
```

On **Mac/Windows**, `host.docker.internal` works by default.

## 🎯 Next Steps

1. ✅ Start monitoring stack
2. ✅ View pre-configured dashboard
3. 📊 Create resources to see metrics update
4. 🔧 Customize dashboard to your needs
5. 🚨 Set up alerts (see monitoring/README.md)
6. 📈 Add custom metrics (see monitoring/ARCHITECTURE.md)

---

**Questions?** Check the detailed docs in the `monitoring/` folder.
