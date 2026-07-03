.PHONY: build check lint fix test clean run start stop restart status logs

# Default workspace for development
WORKSPACE ?= ./test-workspace

# Build the Docker image
build:
	docker-compose build

# Build locally (for development)
build-local:
	cargo build --locked -p lucidos-engine -p lucidos-cli

# Run all checks
check: lint
	cargo check --locked

# Run clippy linter
lint:
	cargo clippy --locked --all-targets -- -D warnings

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
	rm -rf test-workspace/data test-workspace/.lucidos

# Clean everything including artifacts (use with caution)
clean-all:
	cargo clean
	rm -rf test-workspace
	docker-compose down --volumes --remove-orphans

# Start Lucidos in Docker (background)
start:
	LUCIDOS_WORKSPACE=$(WORKSPACE) ./scripts/start.sh

# Stop Lucidos Docker container
stop:
	./scripts/stop.sh -w $(WORKSPACE)

# Restart Lucidos Docker container
restart:
	./scripts/restart.sh -w $(WORKSPACE)

# Check status
status:
	./scripts/status.sh

# View logs
logs:
	docker-compose logs -f

# Run in foreground (for development)
run:
	LUCIDOS_WORKSPACE=$(WORKSPACE) ./scripts/start.sh -f

# Run locally without Docker (for development)
run-local:
	rm -rf test-workspace/data test-workspace/.lucidos
	LUCIDOS_WORKSPACE=./test-workspace cargo run --locked -p lucidos-engine

# Build and run fresh
fresh: build
	./scripts/stop.sh -w $(WORKSPACE) || true
	rm -rf $(WORKSPACE)/data $(WORKSPACE)/.lucidos
	LUCIDOS_WORKSPACE=$(WORKSPACE) ./scripts/start.sh
