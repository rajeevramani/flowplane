# Build stage
FROM rust:1.89-slim as builder

# Install system dependencies for building
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    libpq-dev \
    curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy dependency files
COPY Cargo.toml Cargo.lock ./

# Create a dummy main.rs to build dependencies
RUN mkdir src && echo "fn main() {}" > src/main.rs

# Build dependencies (this layer will be cached)
RUN cargo build --release && rm -rf src

# Copy source code
COPY src ./src
COPY migrations ./migrations

# Build the application
RUN touch src/main.rs && cargo build --release

# Runtime stage
FROM debian:trixie-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    libpq5 \
    && rm -rf /var/lib/apt/lists/*

# Create app user
RUN useradd -r -s /bin/false -m -d /app flowplane

WORKDIR /app

# Copy the binary from builder stage
COPY --from=builder /app/target/release/flowplane /usr/local/bin/flowplane

# Copy migrations
COPY --chown=flowplane:flowplane migrations ./migrations

# Change ownership
RUN chown -R flowplane:flowplane /app

# Switch to app user
USER flowplane

# Health check
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:8080/health || exit 1

# Expose ports
# 8080: HTTP API
# 50051: xDS gRPC
EXPOSE 8080 50051

# Set environment variables
ENV RUST_LOG=info
ENV FLOWPLANE_HOST=0.0.0.0
ENV FLOWPLANE_PORT=8080
ENV FLOWPLANE_XDS_PORT=50051

# Run the application
CMD ["flowplane"]