# ─── Stage 1: Builder ─────────────────────────────────────────────────────────
# Chainguard/Wolfi hardened Rust image (-dev variant has shell + build tools).
# Produces a static binary (musl/crt-static) so the runtime needs no libc.
FROM cgr.dev/chainguard/rust:latest-dev AS builder

WORKDIR /app

# Copy manifests first for layer caching
COPY Cargo.toml Cargo.lock build.rs ./

# Copy source tree
COPY src/ ./src/

# Build a static release binary
# target-feature=+crt-static links the C runtime statically (needed for the
# Chainguard static runtime image which has no libc).
RUN RUSTFLAGS='-C target-feature=+crt-static' \
    cargo build --release --locked

# ─── Stage 2: Runtime ─────────────────────────────────────────────────────────
# Minimal, hardened image — no shell, no build tools, no package manager.
# Chainguard static images run as UID 65532 (nonroot) by default.
FROM cgr.dev/chainguard/static:latest

WORKDIR /app

# Copy the statically-linked binary from the builder
COPY --from=builder /app/target/release/total-recall /app/total-recall

# total-recall communicates via stdio (MCP protocol).
# Data dirs are expected to be mounted at /data — map your host paths there.
# See .env.example for runtime configuration.
ENTRYPOINT ["/app/total-recall"]
CMD ["serve"]
