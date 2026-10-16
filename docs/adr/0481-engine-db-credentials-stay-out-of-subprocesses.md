# 0481: Engine database credentials stay out of subprocesses

- **Status**: Accepted
- **Date**: 2026-10-14
- **Amends**: [0356](0356-coding-agent-threads-get-a-built-in-terminal.md)
  decision 5. The built-in terminal gets `LUCIDOS_DATABASE_URL`, not the
  `PG*` variables.

## Context

A user on the macOS DMG lost every thread and preference. A trigger's coding
agent ran a restore script with `STAGING_DB_URL` unset, so
`psql "$STAGING_DB_URL"` became `psql ""`. libpq then fell back to the `PG*`
variables the engine injected into every subprocess. They named the live
workspace database and the cluster superuser, so the script's
`DROP SCHEMA public CASCADE` ran there.

The engine served errors. The user killed it, the gateway respawned it, and
migrations rebuilt a blank schema without a word.

Two leaks made this possible:

- The engine injected `PGUSER`, `PGPASSWORD`, `PGHOST`, `PGPORT` and
  `PGDATABASE` into agent sessions, chat tools, scripts and triggers. Every
  libpq tool reads them as defaults, so an empty connection string meant the
  workspace database.
- The engine's own `DATABASE_URL` stayed in its process env. About fifteen
  spawn sites inherited it: MCP servers, Vite preview, git, builds and more.
  Many ORMs and frameworks read `DATABASE_URL` by default.

## Decision

1. **The engine removes `DATABASE_URL` from its own process env at boot**, right
   after `core::database_url()` caches it and before user variables apply. A
   user variable named `DATABASE_URL` still reaches children.
2. **The engine injects no `PG*` variables.** Its one handle is
   `LUCIDOS_DATABASE_URL`, a name no libpq tool or ORM reads by default.
3. **`LUCIDOS_DATABASE_URL` is read-only by default.** It carries
   `options=-c default_transaction_read_only=on`. A write needs
   `BEGIN READ WRITE` or `SET default_transaction_read_only = off`.
4. **It reaches only the surfaces that had `PG*`**: coding-agent sessions, chat
   `run_bash` / `run_python`, scheduled scripts, triggers and agent background
   tasks.
5. **The engine reserves `LUCIDOS_DATABASE_URL`** and no longer reserves the
   `PG*` names, which now belong to the user.
6. **Existing scripts migrate per ADR 0214**: a `workspace-audit/scripts.md`
   check with remediation, and a release notice that offers the audit.
7. **A migration marker guards boot.** After a successful migration the engine
   writes `<ws>/.lucidos/database-migrated`, holding the database name and the
   cluster's `system_identifier`. A marker for the same database in the same
   cluster, plus a missing `_sqlx_migrations` table, is a terminal boot
   failure. The message says to restore the latest backup, then to ask the
   restored workspace's Lucidos Agent to copy newer files across. It also names
   the marker file to delete to start empty. A missing marker never blocks.
8. **The running engine checks too.** The database health probe also asks
   `to_regclass('_sqlx_migrations')`, through `search_path` as sqlx created it.
   Two confirmed misses in a row, with a marker naming this database, report
   the same terminal failure, and the engine stops. A query error never counts
   as a miss.

## Rationale

- **Fail closed on an empty connection string.** With no `PG*` defaults, `psql ""`
  connects to nothing useful and errors. That alone stops the incident.
- **One choke point beats fifteen.** Scrubbing the engine's own env covers every
  current and future spawn site. Clearing the env per spawn site would miss the
  next one.
- **An unusual name opts in on purpose.** A tool reads `LUCIDOS_DATABASE_URL`
  only when someone wrote it into the command. The command then names the
  variable, never the password, so the persisted tool call stays clean.
- **Read-only by default makes an accident loud.** DDL and DML fail with
  `read-only transaction`. Postgres also refuses `DROP DATABASE` there, which
  covers the shared superuser's reach into other workspaces by accident.
- **The marker turns a silent blank boot into a clear stop.** Without it,
  migrations rebuild an empty schema and the user sees a fresh workspace with no
  history. A missing marker means "unknown", and the boot-failure doctrine says
  nothing when in doubt.
- **Message only, prompt-first.** Restore from backup already exists in the
  picker. Copying newer files is a task for the restored workspace's agent, not
  for a new button.

## Consequences

- Scripts and triggers that ran bare `psql` against the workspace database break
  until they use `psql "$LUCIDOS_DATABASE_URL"`. The audit check and the release
  notice point them at the fix.
- A script that writes must say so with `BEGIN READ WRITE`.
- Flows that recreate a database under a surviving `.lucidos/` must delete the
  marker, or they hit the wiped-database stop.
- The guard is against accidents, not deliberate misuse. A same-user process can
  still read the engine's credentials if it tries.

## Alternatives considered

- **A restricted Postgres role for subprocesses.** Rejected: packaged auth is
  peer auth on a socket only the user can open, so any same-user process can
  pick the `lucidos` role anyway. A role is no boundary there, and the read-only
  default gives the same accident guard.
- **Keep `PG*` but point them at a read-only role or options.** Rejected: `PG*`
  still turns an empty or mistyped connection string into the workspace
  database, which is the shape of the incident.
- **Clear the env at each spawn site.** Rejected: fifteen sites today, and the
  next one would forget.
- **An in-app "start with an empty database" button.** Rejected for message
  only: deleting the marker file is rare and deliberate.
- **Restore copies newer files itself.** Rejected: the restored workspace's
  agent can do that from a prompt, which keeps restore simple.
