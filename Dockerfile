# ---- Build stage for the React frontend ----
FROM node:20-alpine AS frontend-build
WORKDIR /frontend
COPY frontend/package.json frontend/package-lock.json* ./
RUN npm install --no-audit --no-fund || npm install --no-audit --no-fund
COPY frontend/ ./
RUN npm run build

# ---- Build stage for the Rust workspace ----
# Single-step build: copy everything, build once. Simpler and correct — no
# stub binaries, no cache ambiguity. The dependency compile takes ~3min in CI
# but is cached by Docker layer hashing on Cargo.lock.
FROM rust:1-bookworm AS rust-build
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY Cargo.toml Cargo.lock* ./
COPY crates/ ./crates/
RUN cargo build --release -p relay-panel -p relay-node

# ---- Panel runtime ----
FROM debian:bookworm-slim AS panel
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=rust-build /app/target/release/relay-panel /app/relay-panel
COPY --from=frontend-build /frontend/dist /app/public
VOLUME ["/app/data"]
EXPOSE 18888
ENV DATABASE_URL="sqlite:/app/data/data.db?mode=rwc" \
    LISTEN="0.0.0.0:18888" \
    PUBLIC_DIR="/app/public"
CMD ["./relay-panel"]

# ---- Node runtime ----
FROM debian:bookworm-slim AS node
# v1.0.5: iproute2 provides the `ip` command used to resolve an interface's
# IPv4 address when OUTBOUND_INTERFACE is set. Without it, multi-NIC egress
# selection by interface name would fail. ca-certificates is for HTTPS to the
# panel.
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates iproute2 && \
    rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=rust-build /app/target/release/relay-node /app/relay-node
# ENTRYPOINT (not CMD) so `docker run image --version` appends the flag to the
# binary instead of replacing it. With CMD, `docker run image --version` tries
# to execute "--version" as a program (the release verify job hit this).
ENTRYPOINT ["./relay-node"]
