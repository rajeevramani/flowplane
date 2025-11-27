# Set timestamp
  TIMESTAMP=$(date +%Y%m%d-%H%M%S)
  LOG_FILE="./data/logs/flowplane-${TIMESTAMP}.log"

cargo clean

# Optional arg "new-db=y" clears and recreates the local DB before starting
  if [ "$1" = "y" ]; then
    echo "Creating new local Flowplane database..."
    rm -f /Users/rajeevramani/workspace/projects/flowplane/data/flowplane*
    touch /Users/rajeevramani/workspace/projects/flowplane/data/flowplane.db
    echo "New database created."
  fi

  echo "Starting Flowplane Control Plane..."
  echo "Logs will be written to: ${LOG_FILE}"
  echo "Press Ctrl+C to stop"
  echo ""

  

  FLOWPLANE_DATABASE_URL=sqlite://./data/flowplane.db \
  FLOWPLANE_UI_ORIGIN=http://localhost:5173 \
  # FLOWPLANE_OTLP_ENDPOINT=http://localhost:4317 \
  FLOWPLANE_LOG_LEVEL=info \
  FLOWPLANE_ENABLE_METRICS=false \
  FLOWPLANE_API_BIND_ADDRESS=127.0.0.1 \
  FLOWPLANE_API_PORT=8080 \
  FLOWPLANE_XDS_PORT=18000 \
  cargo run --bin flowplane 2>&1 | tee "${LOG_FILE}"
