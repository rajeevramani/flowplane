#!/bin/bash
# Quick start script for Flowplane monitoring stack

set -e

echo "🚀 Starting Flowplane Monitoring Stack..."
echo ""

# Check if Docker is running
if ! docker info > /dev/null 2>&1; then
    echo "❌ Error: Docker is not running. Please start Docker first."
    exit 1
fi

# Start the monitoring stack
echo "📊 Starting Prometheus and Grafana..."
docker-compose -f docker-compose-monitoring.yml up -d

# Wait for services to be healthy
echo ""
echo "⏳ Waiting for services to start..."
sleep 5

# Check if services are running
if docker ps | grep -q "flowplane-prometheus"; then
    echo "✅ Prometheus is running"
else
    echo "❌ Prometheus failed to start"
    exit 1
fi

if docker ps | grep -q "flowplane-grafana"; then
    echo "✅ Grafana is running"
else
    echo "❌ Grafana failed to start"
    exit 1
fi

echo ""
echo "🎉 Monitoring stack is ready!"
echo ""
echo "📍 Access Points:"
echo "   • Grafana Dashboard: http://localhost:3000"
echo "     (username: admin, password: admin)"
echo ""
echo "   • Prometheus UI: http://localhost:9091"
echo ""
echo "   • Flowplane Metrics: http://localhost:9090/metrics"
echo ""
echo "📈 Pre-configured Dashboard:"
echo "   Navigate to: Dashboards → Flowplane → Team-Based Metrics"
echo ""
echo "💡 Tips:"
echo "   • Make sure Flowplane is running: cargo run --bin flowplane"
echo "   • Check Prometheus targets: http://localhost:9091/targets"
echo "   • View logs: docker-compose -f docker-compose-monitoring.yml logs -f"
echo ""
echo "🛑 To stop: docker-compose -f docker-compose-monitoring.yml down"
