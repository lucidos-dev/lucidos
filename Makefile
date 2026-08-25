.PHONY: build check lint lint-eval lint-fmt lint-rust lint-rust-clippy lint-shell fix test test-eval test-full clean run start stop restart status logs

# Run a heavy build under a *build slot*, so parallel coding-agent worktrees
# cannot pile N full compiles onto one host. Degrades to a plain run when the
# `lucidos` binary is not there, so a fresh `git clone` still builds (ADR 0070).
BUILD_SLOT := ./scripts/with-build-slot.sh

# Default workspace for development
WORKSPACE ?= ./test-workspace

# THE canonical clippy invocation — single source of truth for the lint gate.
# Consumed by `lint-rust-clippy` below, which `lint-rust` runs inside a build
# slot (and therefore by `lint` / `check`).
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

# Run every linter. THE clean-build lint gate, ordered cheapest first so a
# finding surfaces immediately instead of after a full clippy pass: shell takes
# under a second, the fmt check a few seconds, clippy minutes.
lint: lint-shell lint-eval lint-fmt lint-rust

# Fail if any tracked Rust file is not rustfmt-clean.
#
# The tree was swept clean in one mechanical commit and this keeps it that way.
# Formatting was pure convention before, enforced by nothing, and 424 of 614
# tracked .rs files had drifted.
#
#   --all     Explicit rather than relying on the virtual-manifest default, for
#             the same reason CLIPPY_FLAGS spells out --workspace.
#   --check   Report and exit non-zero; never rewrite. `make fmt` is the fix.
#
# No --locked here, and that is not an oversight in ADR 0020's "every cargo call
# is --locked" rule: `cargo fmt` REJECTS the flag (`error: unexpected argument
# '--locked' found`) because it resolves no dependencies, so there is no
# lockfile for it to drift against.
#
# There is deliberately NO rustfmt.toml. Stock defaults are reproducible because
# rust-toolchain.toml pins the toolchain (and rustfmt with it), and a config file
# would be a live footgun: on a stable channel rustfmt WARNS and continues on a
# nightly-only key rather than failing, so an inert setting reads as an active
# one.
# The recipe is `@`-quiet and echoes the bare command itself, so the log reads
# like the other targets' instead of printing the `||` failure-hint wrapper.
lint-fmt:
	@echo "cargo fmt --all --check"
	@cargo fmt --all --check || { \
		echo ""; \
		echo "Not rustfmt-clean. Run \`make fmt\` (or \`make fix\`) and commit the result."; \
		exit 1; \
	}

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
# Both passes run inside ONE build slot rather than one each, so a queued lint
# is admitted once and does not re-queue between them. `lint-shell` and
# `lint-fmt` stay outside the slot: seconds of work must not wait behind
# minutes of clippy.
lint-rust:
	@$(BUILD_SLOT) --label "make lint" -- $(MAKE) --no-print-directory lint-rust-clippy

# The clippy pair itself. Reached through `lint-rust`, which is what holds the
# slot; invoking this target directly deliberately skips it.
lint-rust-clippy:
	cargo clippy $(CLIPPY_FLAGS) --all-features -- -D warnings
	cargo clippy $(CLIPPY_FLAGS) -- -D warnings

# Run ShellCheck over every tracked shell script (discovery + flags live in the
# script and .shellcheckrc, so a hand-run `shellcheck foo.sh` agrees with this).
lint-shell:
	./scripts/lint-shell.sh

# I4 (ADR 0087 decision 15): the context-mode eval is a binary, never a test.
# Cheap (a few greps), so it sits beside lint-shell rather than behind clippy.
# The script's header is the authority on what it checks and why.
lint-eval:
	./scripts/check-eval-not-a-test.sh

# Auto-fix linting issues
fix:
	cargo fix --allow-dirty --allow-staged
	cargo fmt --all

# Format code. The remediation for a `lint-fmt` failure, so it takes the same
# --all as the check: a `make fmt` that formatted less than the gate inspects
# would leave `make lint` red with nothing left to do about it.
fmt:
	cargo fmt --all

# Run tests
test: test-eval
	./scripts/test-engine.sh

# Full test suite
test-full: test-eval
	./scripts/test-engine.sh --full

# The context-mode eval crate's own unit tests. Needs no Postgres and runs in
# well under a second, so it goes first: a broken invariant should surface
# before the engine suite's two minutes, not after.
#
# Running the eval crate under `cargo test` is safe BY CONSTRUCTION, and this
# is not in tension with I4. `lint-eval` proves nothing a test can reach boots
# a workspace, drives a thread, drops a database or calls a provider. Without
# this line the crate's invariant tests would never run in the gate, which is
# the same as not having written them.
test-eval:
	cargo test --locked -p lucidos-eval

# Clean build artifacts (preserves workspace artifacts)
#
# Deliberately does NOT remove `.launch/`, the published launch binaries
# (ADR 0063). That directory holds the `lucidos` CLI the engine puts on PATH for
# every spawned trigger and coding-agent session, so removing it here would
# disable a running workspace exactly the way the `cargo clean` under it used
# to. Do not "fix" this by adding `.launch` below: use `clean-all`, which is the
# deliberate reclaim, or `rm -rf .launch` directly.
clean:
	cargo clean
	rm -rf test-workspace/data test-workspace/.lucidos

# Clean everything including artifacts (use with caution)
#
# Includes `.launch/`, which `clean` deliberately spares. Stop your workspaces
# first: a running engine loses the CLI it hands to its subprocesses, and the
# next `web-dev.sh -b` is what puts it back.
clean-all:
	cargo clean
	rm -rf .launch
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
