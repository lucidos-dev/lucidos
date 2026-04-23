# Build stage
FROM rust:1.83 AS builder

WORKDIR /app
COPY . .

# Build with release optimizations.
# cognos-cli is built alongside the engine so the `cognos` binary lands in
# /usr/bin (next to cognos-engine), matching what `cognos_cli_dir()` expects
# at runtime when prepending to spawned CC sessions' PATH.
RUN cargo build -p cognos-engine -p cognos-cli --release

# Runtime stage - single container with PostgreSQL + pgvector + CognOS
FROM debian:bookworm-slim

# Install PostgreSQL, pgvector, and other dependencies
RUN apt-get update && apt-get install -y \
    # PostgreSQL
    postgresql-16 \
    postgresql-16-pgvector \
    # Python
    python3 \
    python3-pip \
    python3-venv \
    # Libraries
    libpq5 \
    libssl3 \
    ca-certificates \
    # PDF text extraction
    poppler-utils \
    # OCR for scanned PDFs
    tesseract-ocr \
    tesseract-ocr-eng \
    tesseract-ocr-nor \
    # Image processing for OCR
    imagemagick \
    ghostscript \
    # Utilities
    sudo \
    && rm -rf /var/lib/apt/lists/* \
    # Allow ImageMagick to process PDFs
    && sed -i 's/rights="none" pattern="PDF"/rights="read|write" pattern="PDF"/' /etc/ImageMagick-6/policy.xml 2>/dev/null || true

# Create workspace directory
RUN mkdir -p /workspace/artifacts /workspace/.cognos /workspace/data/postgres

# Copy the built binaries — `cognos` must live next to `cognos-engine` because
# the engine resolves it relative to its own current_exe.
COPY --from=builder /app/target/release/cognos-engine /usr/bin/cognos-engine
COPY --from=builder /app/target/release/cognos /usr/bin/cognos

# Copy entrypoint script
COPY docker-entrypoint.sh /usr/bin/docker-entrypoint.sh
RUN chmod +x /usr/bin/docker-entrypoint.sh

WORKDIR /workspace

# Expose API port
EXPOSE 3000

ENTRYPOINT ["/usr/bin/docker-entrypoint.sh"]
