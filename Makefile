.PHONY: build check lint lint-rust lint-shell fix test clean run start stop restart status logs

# Default workspace for development
WORKSPACE ?= ./test-workspace

# THE canonical clippy invocation — single source of truth for the lint gate.
# Consumed by `lint-rust` below (and therefore by `lint` / `check`).
# `/harden` Phase 4.5 (the per-change gate) and
# `.claude/skills/clean-build/SKILL.md` both run `make lint`; neither repeats
# these flags. Change them here only.
#
#   --locked        ADR 0020: every cargo call in scripts/** and this Makefile is
#                   --locked, so the gate ERRORS on Cargo.toml<->Cargo.lock drift
#                   instead of silently rewriting the lockfile.
#   --workspace     Explicit rather than relying on the virtual-manifest default.
#   --all-targets   lib, bins, tests, examples, benches — catches dead_code /
#                   unused that a lib-only lint misses.
#
# Deliberately NOT --release: clippy's lint set is profile-independent, a release
# lint uses a separate target dir (no cache sharing with cargo check/test), and
# debug lints MORE code because cfg(debug_assertions) blocks compile.
CLIPPY_FLAGS := --locked --workspace --all-targets

# Build the Docker image
build:
	docker-compose build

# Build locally (for development)
build-local:
	cargo build --locked -p lucidos-engine -p lucidos-cli

# Run all checks
check: lint
	cargo check --locked

# Run every linter — THE clean-build lint gate. Shell first: it takes under a
# second, so a shell finding surfaces immediately instead of after a full
# clippy pass.
lint: lint-shell lint-rust

# Run clippy linter — TWICE, once per feature configuration, because neither
# one alone lints the whole tree (see CLIPPY_FLAGS for the shared flags).
#
#   --all-features  lucidos-engine gates real code behind `e2e-test-hooks` (the
#                   e2e workspace builds with it) and `real-embedder-tests`;
#                   without this pass they are never linted.
#   default         `e2e-test-hooks` compiles the PRODUCTION push transport OUT
#                   — `scheduler/push.rs` is the repo's only `cfg(not(feature
#                   …))`, and `fan_out_to_web_push` (VAPID, the real APNs/FCM
#                   send loop) exists only when the feature is OFF. An
#                   --all-features-only gate is blind to ~100 lines of shipping
#                   code; verified by planting a `ptr_arg` in that region, which
#                   --all-features passed and this pass caught.
#
# Cargo fingerprints artifacts per feature set, so both stay cached in target/
# and alternating between them does not thrash — the second pass is near-free
# once warm.
lint-rust:
	cargo clippy $(CLIPPY_FLAGS) --all-features -- -D warnings
	cargo clippy $(CLIPPY_FLAGS) -- -D warnings

# Run ShellCheck over every tracked shell script (discovery + flags live in the
# script and .shellcheckrc, so a hand-run `shellcheck foo.sh` agrees with this).
lint-shell:
	./scripts/lint-shell.sh

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
