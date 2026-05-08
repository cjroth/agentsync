# syntax=docker/dockerfile:1.7

FROM rust:1.89-slim-bookworm AS builder
WORKDIR /app

RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential pkg-config perl \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY tests ./tests

RUN cargo build --release -p agentsync-cli --bin agentsync

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/agentsync /usr/local/bin/agentsync

ENV HOME=/data \
    AGENTSYNC_LOG=info

WORKDIR /data/vault
EXPOSE 1234

CMD ["sh", "-c", "mkdir -p /data/vault && cd /data/vault && { [ -f .agentsync/config.toml ] || agentsync init; } && exec agentsync watch --listen 0.0.0.0:${PORT:-1234}"]
