.PHONY: build check lint fix test clean run start stop restart status logs

# Default workspace for development
WORKSPACE ?= ./test-workspace

# Build the Docker image
build:
	docker-compose build

# Build locally (for development)
build-local:
	cargo build -p cognos-engine -p cognos-cli

# Run all checks
check: lint
	cargo check

# Run clippy linter
lint:
	cargo clippy --all-targets -- -D warnings

# Auto-fix linting issues
fix:
	cargo fix --allow-dirty --allow-staged
	cargo fmt

# Format code
fmt:
	cargo fmt

# Run tests
test:
	./scripts/test-engine.sh

# Full test suite
test-full:
	./scripts/test-engine.sh --full

# Clean build artifacts (preserves workspace artifacts)
clean:
	cargo clean
	rm -rf test-workspace/data test-workspace/.cognos

# Clean everything including artifacts (use with caution)
clean-all:
	cargo clean
	rm -rf test-workspace
	docker-compose down --volumes --remove-orphans

# Start CognOS in Docker (background)
start:
	COGNOS_WORKSPACE=$(WORKSPACE) ./scripts/start.sh

# Stop CognOS Docker container
stop:
	./scripts/stop.sh

# Restart CognOS Docker container
restart:
	./scripts/restart.sh

# Check status
status:
	./scripts/status.sh

# View logs
logs:
	docker-compose logs -f

# Run in foreground (for development)
run:
	COGNOS_WORKSPACE=$(WORKSPACE) ./scripts/start.sh -f

# Run locally without Docker (for development)
run-local:
	rm -rf test-workspace/data test-workspace/.cognos
	COGNOS_WORKSPACE=./test-workspace cargo run -p cognos-engine

# Build and run fresh
fresh: build
	./scripts/stop.sh || true
	rm -rf $(WORKSPACE)/data $(WORKSPACE)/.cognos
	COGNOS_WORKSPACE=$(WORKSPACE) ./scripts/start.sh
