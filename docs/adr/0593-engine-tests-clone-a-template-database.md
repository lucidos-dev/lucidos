# 0593: Engine tests clone a migrated template database

- **Status**: Accepted
- **Date**: 2026-10-28

## Context

Every DB-backed engine test called `setup_test_db`, which ran `CREATE DATABASE`
and then all 267 migrations. About 2,700 call sites did this, and the full
engine lib suite took 535 s in a warm worktree. The test container
`lucidos-pg-test` also ran with full durability, although nobody keeps its
data.

Plan: `docs/plans/2026-10-28-harden-suites-proportional-to-diff.md`.

## Decision

1. **`setup_test_db` clones a migrated template.** The template is
   `lucidos_template_<hash>`, and the hash covers every migration's version and
   checksum from `sqlx::migrate!()`. The first test that needs it builds it with
   the same migrations as before. Each test still gets its own fresh database.
2. **Advisory locks guard the template across worktrees.** A clone holds
   `hashtext(template)` shared, and a build or a stale-template drop holds it
   exclusive. A template older than a week is dropped at the next build.
3. **The test container runs without durability.** `test-engine.sh` sets
   `fsync`, `synchronous_commit` and `full_page_writes` off. All three reload
   without a restart, so an existing container gets them through
   `ALTER SYSTEM` and `pg_reload_conf()`.

## Rationale

The full engine lib suite fell from 535 s to 289 s, with every test passing.
Isolation is unchanged: no two tests share a database. The template name stays
outside `lucidos_test_%`, so the orphan sweep never touches it. A changed
migration set gets a new name, so a stale schema cannot be cloned.

Durability only protects data across a crash. A crash here loses throwaway
databases, and `--fresh` already recreates the container.

## Consequences

- Every Rust change pays about half the engine-test time it did.
- A branch that edits a migration builds its own template once, about one
  migration run, and leaves the other branches' templates alone.
- The clone uses `STRATEGY FILE_COPY` on Postgres 15 and later. On an older
  server set through `TEST_DATABASE_URL`, it uses a plain `TEMPLATE` clone.
- A clone's `_sqlx_migrations.installed_on` is when the template was built,
  which can be hours ago. A test that times rows against it must time every
  row against it, never against `now()`. `migration_tests.rs` does.

## Alternatives considered

- **Share one database between tests, with a transaction per test.** Faster,
  but many tests commit, open several connections, or listen for
  notifications. Isolation would change, not only speed.
- **Keep migrating per test and only drop durability.** It halves setup, but the
  benchmark measured the clone at 3.6 s against 9.6 s per setup, on top.
