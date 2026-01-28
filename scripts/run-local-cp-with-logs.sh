# Set timestamp
  TIMESTAMP=$(date +%Y%m%d-%H%M%S)
  LOG_FILE="./data/logs/flowplane-${TIMESTAMP}.log"

cd ui

# Build the UI
  echo "Building Flowplane UI..."
  npm run build

cd ..

cargo clean

# Optional arg "new-db=y" clears and recreates the local DB before starting
  if [ "$1" = "y" ]; then
    echo "Creating new local Flowplane database..."
    rm -f /Users/rajeevramani/workspace/projects/flowplane/data/development/flowplane*
    touch /Users/rajeevramani/workspace/projects/flowplane/data/development/flowplane.db
    echo "New database created."
  fi

  echo "Starting Flowplane Control Plane..."
  echo "Logs will be written to: ${LOG_FILE}"
  echo "Press Ctrl+C to stop"
  echo ""

  # Use a stable encryption key for development (stored in .dev-encryption-key)
  # Generate once if not exists, then reuse across restarts
  KEY_FILE="./data/.dev-encryption-key"
  if [ ! -f "$KEY_FILE" ]; then
    echo "Generating new development encryption key..."
    openssl rand -base64 32 > "$KEY_FILE"
    chmod 600 "$KEY_FILE"
  fi
  ENCRYPTION_KEY=$(cat "$KEY_FILE")

  FLOWPLANE_DATABASE_URL=sqlite://./data/development/flowplane.db \
  FLOWPLANE_UI_ORIGIN=http://localhost:5173 \
  FLOWPLANE_OTLP_ENDPOINT=http://localhost:4317 \
  FLOWPLANE_LOG_LEVEL=info \
  FLOWPLANE_ENABLE_METRICS=false \
  FLOWPLANE_API_BIND_ADDRESS=127.0.0.1 \
  FLOWPLANE_API_PORT=8080 \
  FLOWPLANE_XDS_PORT=18000 \
  FLOWPLANE_SECRET_ENCRYPTION_KEY="${ENCRYPTION_KEY}" \
  FLOWPLANE_VAULT_ADDR=http://localhost:8200 \
  FLOWPLANE_VAULT_TOKEN=flowplane-dev-token \
  cargo run --bin flowplane 2>&1 | tee "${LOG_FILE}"
