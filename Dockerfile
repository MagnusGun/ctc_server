# Multi-stage Dockerfile for CTC Server (ARM64)
# Uses cargo-chef for optimized dependency caching

# Stage 1: Planner - Generate recipe.json for dependency caching
FROM rust:latest AS planner
WORKDIR /app
RUN cargo install cargo-chef
COPY Cargo.toml Cargo.lock ./
COPY server/Cargo.toml ./server/
# Create minimal source structure for cargo-chef to analyze workspace
RUN mkdir -p server/src && echo "fn main() {}" > server/src/main.rs
RUN cargo chef prepare --recipe-path recipe.json

# Stage 2: Cacher - Build and cache dependencies
FROM rust:latest AS cacher
WORKDIR /app

# Install build dependencies for serial port libraries
RUN apt-get update && apt-get install -y \
    libudev-dev \
    pkg-config \
    && rm -rf /var/lib/apt/lists/*

# Install cargo-chef
RUN cargo install cargo-chef

# Copy recipe and build dependencies
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

# Stage 3: Builder - Build the actual application
FROM rust:latest AS builder
WORKDIR /app

# Install build dependencies
RUN apt-get update && apt-get install -y \
    libudev-dev \
    pkg-config \
    && rm -rf /var/lib/apt/lists/*

# Copy cached dependencies from cacher stage
COPY --from=cacher /app/target target
COPY --from=cacher /usr/local/cargo /usr/local/cargo

# Copy source code
COPY Cargo.toml Cargo.lock ./
COPY server ./server

# Build the release binary
RUN cargo build --release -p server

# Stage 4: Runtime - Minimal runtime image
FROM debian:bookworm-slim AS runtime
WORKDIR /app

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    libudev1 \
    ca-certificates \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user with dialout group for serial port access
RUN groupadd -r ctc --gid=1000 && \
    useradd -r -g ctc --uid=1000 --home-dir=/app --shell=/bin/bash ctc && \
    usermod -a -G dialout ctc

# Copy binary from builder
COPY --from=builder /app/target/release/server /app/server

# Copy default config template
COPY config.toml.example /app/config.toml.default

# Set ownership
RUN chown -R ctc:ctc /app

# Switch to non-root user
USER ctc

# Expose HTTP port
EXPOSE 3000

# Health check - test if the API is responding
HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:3000/api/v1/temperature/outdoor || exit 1

# Set default environment variables
ENV RUST_LOG=info

# Entrypoint
ENTRYPOINT ["/app/server"]

# Default CMD (can be overridden to specify serial port)
CMD []
