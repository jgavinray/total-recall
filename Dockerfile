# ─── Stage 1: Builder ─────────────────────────────────────────────────────────
# Use Rust on Ubuntu 24.04 to match hyper01's glibc
FROM ubuntu:24.04 AS builder

RUN apt-get update && apt-get install -y \
    curl \
    build-essential \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Install Rust toolchain
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
ENV PATH="/root/.cargo/bin:${PATH}"

WORKDIR /app

# Copy manifests first for layer caching
COPY Cargo.toml Cargo.lock build.rs ./

# Copy source tree
COPY src/ ./src/

# Build release binary
RUN cargo build --release --locked

# ─── Stage 2: Runtime ─────────────────────────────────────────────────────────
# Ubuntu 24.04 to match hyper01's glibc 2.39
FROM ubuntu:24.04

RUN apt-get update && apt-get install -y \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy the binary from the builder
COPY --from=builder /app/target/release/total-recall /app/total-recall

# Data dirs mounted at /data from host
# Config via TOTAL_RECALL_CONFIG env var
ENTRYPOINT ["/app/total-recall"]
CMD ["serve"]
