# Multi-stage Dockerfile for CTC Server (ARM64 Debian slim)
# Uses cross-compilation from x86_64 to avoid QEMU emulation issues

# Stage 1: Planner - Generate recipe.json for dependency caching
FROM --platform=$BUILDPLATFORM lukemathwalker/cargo-chef:latest AS planner
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY server/Cargo.toml ./server/
COPY smartgrid_test/Cargo.toml ./smartgrid_test/
# Create minimal source structure for cargo-chef to analyze workspace
RUN mkdir -p server/src && echo "fn main() {}" > server/src/main.rs
RUN mkdir -p smartgrid_test/src && echo "fn main() {}" > smartgrid_test/src/main.rs
RUN cargo chef prepare --recipe-path recipe.json

# Stage 2: Cacher - Build and cache dependencies (cross-compile to ARM64)
FROM --platform=$BUILDPLATFORM lukemathwalker/cargo-chef:latest AS cacher
WORKDIR /app

# Install cross-compilation toolchain for ARM64
RUN apt-get update && apt-get install -y \
    gcc-aarch64-linux-gnu \
    g++-aarch64-linux-gnu \
    pkg-config \
    cmake \
    && rm -rf /var/lib/apt/lists/*

# Add ARM64 target
RUN rustup target add aarch64-unknown-linux-gnu

# Configure cargo for cross-compilation
ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc
ENV CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc
ENV CXX_aarch64_unknown_linux_gnu=aarch64-linux-gnu-g++

# Copy recipe and build dependencies
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --target aarch64-unknown-linux-gnu -p server --recipe-path recipe.json

# Stage 3: Builder - Build the actual application (cross-compile to ARM64)
FROM --platform=$BUILDPLATFORM lukemathwalker/cargo-chef:latest AS builder
WORKDIR /app

# Install cross-compilation toolchain for ARM64
RUN apt-get update && apt-get install -y \
    gcc-aarch64-linux-gnu \
    g++-aarch64-linux-gnu \
    pkg-config \
    cmake \
    && rm -rf /var/lib/apt/lists/*

# Add ARM64 target
RUN rustup target add aarch64-unknown-linux-gnu

# Configure cargo for cross-compilation
ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc
ENV CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc
ENV CXX_aarch64_unknown_linux_gnu=aarch64-linux-gnu-g++

# Copy cached dependencies from cacher stage
COPY --from=cacher /app/target target
COPY --from=cacher /usr/local/cargo /usr/local/cargo

# Copy source code (excluding static files to avoid rebuilds on static changes)
COPY Cargo.toml Cargo.lock ./
COPY server/Cargo.toml ./server/
COPY server/src ./server/src
# Create minimal smartgrid_test structure for workspace resolution (not built)
COPY smartgrid_test/Cargo.toml ./smartgrid_test/
RUN mkdir -p smartgrid_test/src && echo "fn main() {}" > smartgrid_test/src/main.rs

# Build the release binary and strip it
RUN cargo build --release --target aarch64-unknown-linux-gnu -p server \
    && aarch64-linux-gnu-strip /app/target/aarch64-unknown-linux-gnu/release/server

# Stage 4: Runtime - Debian trixie-slim (glibc 2.38+)
FROM --platform=linux/arm64 debian:trixie-slim AS runtime
WORKDIR /app

# Install minimal runtime dependencies and create user
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    wget \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd -g 1000 ctc \
    && useradd -u 1000 -g ctc -d /app -s /sbin/nologin ctc \
    && usermod -aG dialout ctc

# Copy binary from builder with ownership set
COPY --from=builder --chown=1000:1000 /app/target/aarch64-unknown-linux-gnu/release/server /app/server

# Copy static files directly from context with ownership
COPY --chown=1000:1000 server/static /app/static

# Copy default config template with ownership
COPY --chown=1000:1000 config.toml.example /app/config.toml.default

# Switch to non-root user
USER ctc

# Expose HTTP port
EXPOSE 3000

# Set default environment variables
ENV RUST_LOG=info

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD wget -q --spider http://localhost:3000/api/v1/temperature/outdoor || exit 1

# Entrypoint
ENTRYPOINT ["/app/server"]

# Default CMD (can be overridden to specify serial port)
CMD []
