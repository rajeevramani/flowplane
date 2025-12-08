#!/bin/bash
# Run Flowplane Control Plane in secure mode (mTLS enabled)
#
# Prerequisites:
#   - Vault running: docker-compose -f docker-compose-mtls-dev.yml up -d
#   - PKI configured: ./scripts/setup-vault-pki.sh
#
# Usage:
#   ./scripts/run-local-cp-mtls.sh        # Normal start
#   ./scripts/run-local-cp-mtls.sh y      # Reset database first

# Set timestamp
TIMESTAMP=$(date +%Y%m%d-%H%M%S)
LOG_FILE="./data/logs/flowplane-mtls-${TIMESTAMP}.log"

# Ensure logs directory exists
mkdir -p ./data/logs

# Optional arg "y" clears and recreates the local DB before starting
if [ "$1" = "y" ]; then
  echo "Creating new local Flowplane database..."
  rm -f /Users/rajeevramani/workspace/projects/flowplane/data/flowplane*
  touch /Users/rajeevramani/workspace/projects/flowplane/data/flowplane.db
  echo "New database created."
fi

echo "Starting Flowplane Control Plane (mTLS ENABLED)..."
echo "Logs will be written to: ${LOG_FILE}"
echo ""
echo "Vault: http://localhost:8200"
echo "PKI Mount: pki_int_proxies"
echo "Trust Domain: flowplane.local"
echo ""
echo "Press Ctrl+C to stop"
echo ""

# Standard Vault environment variables
export VAULT_ADDR=http://localhost:8200
export VAULT_TOKEN=flowplane-dev-token

# Flowplane configuration
FLOWPLANE_DATABASE_URL=sqlite://./data/development/flowplane.db \
FLOWPLANE_UI_ORIGIN=http://localhost:5173 \
FLOWPLANE_OTLP_ENDPOINT=http://localhost:4317 \
FLOWPLANE_LOG_LEVEL=info \
FLOWPLANE_ENABLE_METRICS=false \
FLOWPLANE_API_BIND_ADDRESS=127.0.0.1 \
FLOWPLANE_API_PORT=8080 \
FLOWPLANE_XDS_PORT=18000 \
FLOWPLANE_VAULT_PKI_MOUNT_PATH=pki_int_proxies \
FLOWPLANE_SPIFFE_TRUST_DOMAIN=flowplane.local \
FLOWPLANE_VAULT_PKI_ROLE=envoy-proxy \
cargo run --bin flowplane 2>&1 | tee "${LOG_FILE}"
