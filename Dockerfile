# Build stage
FROM rust:1.83 AS builder

WORKDIR /app
COPY . .

# Build with release optimizations.
# lucidos-cli is built alongside the engine so the `lucidos` binary lands in
# /usr/bin (next to lucidos-engine), matching what `lucidos_cli_dir()` expects
# at runtime when prepending to spawned CC sessions' PATH.
# `--locked` per ADR 0020: the image must fail on Cargo.toml/Cargo.lock drift
# rather than resolve fresh dependencies and ship a binary the lockfile never
# described.
RUN cargo build --locked -p lucidos-engine -p lucidos-cli --release

# Runtime stage - single container with PostgreSQL + pgvector + Lucidos
FROM debian:bookworm-slim

# Install PostgreSQL, pgvector, and other dependencies
RUN apt-get update && apt-get install -y \
    # PostgreSQL
    postgresql-18 \
    postgresql-18-pgvector \
    # Python
    python3 \
    python3-pip \
    python3-venv \
    # Libraries
    libpq5 \
    libssl3 \
    ca-certificates \
    # Utilities
    sudo \
    && rm -rf /var/lib/apt/lists/*

# Create workspace directory
RUN mkdir -p /workspace/artifacts /workspace/.lucidos /workspace/data/postgres

# Copy the built binaries — `lucidos` must live next to `lucidos-engine` because
# the engine resolves it relative to its own current_exe.
COPY --from=builder /app/target/release/lucidos-engine /usr/bin/lucidos-engine
COPY --from=builder /app/target/release/lucidos /usr/bin/lucidos

# Copy entrypoint script
COPY docker-entrypoint.sh /usr/bin/docker-entrypoint.sh
RUN chmod +x /usr/bin/docker-entrypoint.sh

WORKDIR /workspace

# Expose API port
EXPOSE 3000

ENTRYPOINT ["/usr/bin/docker-entrypoint.sh"]
