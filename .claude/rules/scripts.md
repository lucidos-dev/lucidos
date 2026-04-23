---
globs:
  - "scripts/**"
  - "**/*.sh"
  - "Makefile"
---

# Scripts & Build

## Dev / runtime scripts

```bash
./scripts/web-dev.sh -w <ws> [-b] [-r]    # Start (-b builds; -r release)
./scripts/tauri-dev.sh -w <ws> [-b]       # Start engine + Tauri window
./scripts/stop.sh -w <ws>                 # Stop a specific workspace
./scripts/status.sh                       # Check running status
./scripts/populate.sh -w <ws> [-c]        # Populate test history
./scripts/new-migration.sh <description>  # Create timestamped migration
```

Always use `web-dev.sh -b` to restart. `scripts/lib/ports.sh` allocates per-workspace ports; engine reverse-proxies to Vite. Postgres containers (`cognos-pg-<cksum>`) stay running when engine stops.

## Build

```bash
cargo build -p cognos-engine --release    # Engine
cd crates/cognos-app && cargo tauri build # Desktop app
```

Dev: native engine + Docker PostgreSQL. Production: single Docker container. Makefile: `make build`, `make test`, `make run`.
