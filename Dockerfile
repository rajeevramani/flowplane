# syntax=docker/dockerfile:1.4
#
# Multi-stage Dockerfile for Flowplane (UI + backend).
#
# Build (default - no cloud features):
#   DOCKER_BUILDKIT=1 docker build -t flowplane .
#
# Build with GCP support:
#   DOCKER_BUILDKIT=1 docker build --build-arg CARGO_FEATURES=gcp -t flowplane:gcp .
#
# Build for Cloud Build (GCP):
#   gcloud builds submit --tag gcr.io/PROJECT_ID/flowplane:VERSION \
#     --build-arg CARGO_FEATURES=gcp .
#
# Run:
#   docker run -p 8080:8080 -p 50051:50051 flowplane

# UI build stage
FROM node:22-slim AS ui-builder

WORKDIR /ui

# Copy package files
COPY ui/package.json ui/package-lock.json ./

# Install dependencies
RUN npm install

# Copy UI source
COPY ui/ ./

# Build static files
RUN npm run build

# Backend build stage
FROM rust:1.92-slim AS builder

# Cargo features compiled into the image.
#
# Default is `dev-oidc` so `flowplane init` (which boots via
# docker-compose-dev.yml with FLOWPLANE_AUTH_MODE=dev) works out of the box —
# dev mode hard-refuses startup without this feature (fp-4n5).
#
# Prod deployers override explicitly, e.g.:
#   docker build --build-arg CARGO_FEATURES=gcp -t flowplane:gcp .
ARG CARGO_FEATURES="dev-oidc"

# Install system dependencies for building. protobuf-compiler is required by
# build.rs (tonic-prost-build compiles proto/flowplane/diagnostics/v1/*.proto,
# added in d479996 for the EnvoyDiagnosticsService scaffold).
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    libpq-dev \
    protobuf-compiler \
    curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy source tree. BuildKit cache mounts below keep the cargo registry and
# target directory warm across rebuilds, so incremental builds stay fast
# without cargo-chef (whose 0.1.77 parser choked on newer crate manifests —
# see fp-n237).
COPY Cargo.toml Cargo.lock build.rs ./
COPY src ./src
COPY crates ./crates
COPY proto ./proto
COPY migrations ./migrations
COPY filter-schemas ./filter-schemas
COPY docker-compose-dev.yml ./

# Cache mounts intentionally omitted: podman/buildah's cache mount semantics
# diverge from BuildKit and produced corrupted crate extractions in practice
# (see fp-n237). Build from scratch each time — reliable beats fast here.
RUN if [ -n "$CARGO_FEATURES" ]; then \
        cargo build --release --bin flowplane --features "$CARGO_FEATURES"; \
    else \
        cargo build --release --bin flowplane; \
    fi && \
    cp target/release/flowplane /app/flowplane

# Runtime stage
FROM debian:trixie-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    libpq5 \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Create app user
RUN useradd -r -s /bin/false -m -d /app flowplane

WORKDIR /app

# Copy the binary from builder stage
COPY --from=builder /app/flowplane /usr/local/bin/flowplane

# Copy migrations
COPY --chown=flowplane:flowplane migrations ./migrations

# Copy filter schemas
COPY --chown=flowplane:flowplane filter-schemas ./filter-schemas

# Copy UI static files from ui-builder stage
COPY --from=ui-builder --chown=flowplane:flowplane /ui/build ./ui/build

# Change ownership
RUN chown -R flowplane:flowplane /app

# Switch to app user
USER flowplane

# Health check using Swagger UI endpoint
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:8080/swagger-ui/ || exit 1

# Expose ports
# 8080: HTTP API + UI
# 50051: xDS gRPC
EXPOSE 8080 50051

# Set environment variables
ENV RUST_LOG=info \
    FLOWPLANE_API_BIND_ADDRESS=0.0.0.0 \
    FLOWPLANE_API_PORT=8080 \
    FLOWPLANE_XDS_BIND_ADDRESS=0.0.0.0 \
    FLOWPLANE_XDS_PORT=50051 \
    FLOWPLANE_UI_DIR=/app/ui/build

# Run the application
CMD ["flowplane", "serve"]
