#!/usr/bin/env bash
# shellcheck disable=SC2034 # every name is read by a script that sources this file
# workspace_constants.sh: values the dev scripts share with the Rust crates.
#
# Shell cannot import a Rust constant, so each value here keeps the name of the
# Rust constant that owns it. `value_pins_tests` in crates/lucidos-gateway and
# crates/lucidos-engine read this file and fail when a value drifts. Change the
# Rust constant and this line together.
#
# Assignments only, so any script or hermetic test can source it.

# lucidos_installs::DEFAULT_DEV_GATEWAY_PORT. LUCIDOS_DEV_GATEWAY_PORT overrides
# it at run time.
DEFAULT_DEV_GATEWAY_PORT="5251"

# The shared Docker cluster: the PG_* and SHARED_DOCKER_* constants in
# crates/lucidos-gateway/src/postgres.rs, which says why each value is what it is.
PG_IMAGE="pgvector/pgvector:pg18"
PG_USER="lucidos"
PG_SHM_SIZE="1g"
PG_MAX_CONNECTIONS="500"
SHARED_DOCKER_CONTAINER="lucidos-pg-shared"
SHARED_DOCKER_VOLUME="lucidos-pg-data-shared"

# lucidos_local_token::SHARED_PG_PASSWORD_FILE: the shared cluster's password,
# under ~/.lucidos. Read it through shared_pg_password (scripts/lib/pg_secret.sh).
SHARED_PG_PASSWORD_FILE="pg-shared-password"

# lucidos_local_token::TOKEN_BYTES and ::MINT_LOCK_SUFFIX: how pg_secret.sh mints
# that file, the same way the gateway does.
TOKEN_BYTES="32"
MINT_LOCK_SUFFIX="lock"

# lucidos_engine::paths::WORKTREES_SUBPATH: where a workspace keeps its
# coding-agent worktrees, relative to its root.
WORKTREES_SUBPATH=".lucidos/worktrees"
