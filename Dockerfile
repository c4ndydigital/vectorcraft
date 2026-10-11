# Multi-stage Dockerfile for VectorCraft (Coolify & Production)

# ==============================================================================
# Stage 1: Build WebAssembly frontend and backend release binary
# ==============================================================================
FROM rust:bookworm AS builder

# Set build environment variables
ENV CARGO_TERM_COLOR=always

# Install WebAssembly target
RUN rustup target add wasm32-unknown-unknown

# Install Trunk binary (pinned stable v0.21.14 with architecture detection)
ARG TRUNK_VERSION=0.21.14
RUN dpkgArch="$(dpkg --print-architecture)" && \
    case "${dpkgArch##*-}" in \
      amd64) TRUNK_ARCH="x86_64-unknown-linux-gnu" ;; \
      arm64) TRUNK_ARCH="aarch64-unknown-linux-gnu" ;; \
      *) echo "Unsupported architecture: ${dpkgArch}" && exit 1 ;; \
    esac && \
    curl -fsSL "https://github.com/trunk-rs/trunk/releases/download/v${TRUNK_VERSION}/trunk-${TRUNK_ARCH}.tar.gz" \
      | tar -xz -C /usr/local/bin && \
    chmod +x /usr/local/bin/trunk

WORKDIR /usr/src/vectorcraft

# Copy workspace configuration and manifests
COPY Cargo.toml Cargo.lock clippy.toml rustfmt.toml ./
COPY crates/ crates/
COPY apps/ apps/
COPY xtask/ xtask/
COPY assets/ assets/

# Build production WebAssembly client bundle into dist/web
RUN cd apps/vectorcraft-web && trunk build --release

# Build production backend server binary
RUN cargo build --release --bin vectorcraft-server

# ==============================================================================
# Stage 2: Minimal hardened runtime image
# ==============================================================================
FROM debian:bookworm-slim AS runner

# Install curl for health checks and ca-certificates for secure outbound connections
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
      ca-certificates \
      curl \
    && rm -rf /var/lib/apt/lists/*

# Create unprivileged application user
RUN groupadd -g 10001 vectorcraft && \
    useradd -u 10001 -g vectorcraft -s /bin/sh -d /app -m vectorcraft

WORKDIR /app

# Copy compiled backend binary from builder
COPY --from=builder /usr/src/vectorcraft/target/release/vectorcraft-server /usr/local/bin/vectorcraft-server

# Copy compiled web assets from builder
COPY --from=builder /usr/src/vectorcraft/dist/web /app/dist/web

# Ensure proper permissions
RUN chown -R vectorcraft:vectorcraft /app && \
    chmod +x /usr/local/bin/vectorcraft-server

# Runtime environment defaults (overridable in Coolify)
ENV PORT=8767
ENV BIND=0.0.0.0
ENV DIST_PATH=/app/dist/web
ENV RUST_LOG=info

# Expose default port
EXPOSE 8767

# Healthcheck monitoring using the Axum /health endpoint
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD curl -f -s http://127.0.0.1:${PORT:-8767}/health || exit 1

# Run as non-root user
USER vectorcraft

# Launch server
CMD ["vectorcraft-server"]
