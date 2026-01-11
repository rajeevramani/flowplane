# syntax=docker/dockerfile:1.4
#
# Combined Dockerfile with cargo-chef optimization
# Builds both UI and backend with optimized caching
#
# Build:
#   DOCKER_BUILDKIT=1 docker build -t flowplane .
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

# Cargo Chef Planner Stage
FROM rust:1.89-slim AS chef
RUN cargo install cargo-chef
WORKDIR /app

# Plan dependencies - creates recipe.json
FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY benches ./benches
RUN cargo chef prepare --recipe-path recipe.json

# Build dependencies (cached layer)
FROM chef AS builder

# Install system dependencies for building
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    libpq-dev \
    curl \
    && rm -rf /var/lib/apt/lists/*

COPY --from=planner /app/recipe.json recipe.json

# Build dependencies with BuildKit cache mounts
# This layer is cached and only rebuilds when Cargo.toml/Cargo.lock change
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/target \
    cargo chef cook --release --recipe-path recipe.json

# Build application
COPY Cargo.toml Cargo.lock ./
COPY benches ./benches
COPY src ./src
COPY migrations ./migrations
COPY filter-schemas ./filter-schemas

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/target \
    cargo build --release && \
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
CMD ["flowplane"]
