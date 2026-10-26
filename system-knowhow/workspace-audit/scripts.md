---
name: Workspace audit: scripts
description: The scripts audit section: CLI usage for data writes, events and API calls, hardcoded machine paths, and database access through LUCIDOS_DATABASE_URL.
sources: ["system-knowhow/lucidos-cli.md", "system-knowhow/best-practices.md", "system-knowhow/plugins.md", "crates/lucidos-cli/src/data.rs", "crates/lucidos-engine/src/core/agent_role.rs"]
---

# Workspace audit section: scripts

One *audit section* of `system-knowhow/workspace-audit`. The root says how to
run it alone or in a full pass, how to merge its receipts, and how to write
the report. This file carries its scan, its checks and its fixes.

## Scan

Append these lines to the root's scan preamble, before its receipts tail:

```bash
scan machine-path "/(Users|home)/[^\"'[:space:]]+" $all
scan db-defaults "(^|[^[:alnum:]_])(psql|pg_dump)([^[:alnum:]_.-]|\$)|psycopg2?\.connect\(|new (pg\.)?(Client|Pool)\(|PG(HOST|PORT|USER|PASSWORD|DATABASE)|(^|[^[:alnum:]_])DATABASE_URL" $all
```

## Checks

For each finding, capture: **location**, **what's wrong**, **which reference owns the rule** (link, don't quote), **suggested fix**.

Per `system-knowhow/lucidos-cli.md`:

- Writes to `data/` go through `lucidos data write`, not raw HTTP and not open-coded paths under `$LUCIDOS_WORKSPACE/data/`. Do not flag a script's own runtime state (a cursor, a last-seen id) written directly under `data/artifacts/<plugin-id>/` or `data/triggers/<slug>/state/`. That is the intended pattern, per `system-knowhow/plugins.md` § "Where a plugin keeps its runtime state".
- Domain events go through `lucidos events emit` / `lucidos events query`.
- External API calls go through `lucidos proxy <name>` when the workspace owns a credential for the service. The patterns are the `cross-cutting` section's. A script adds one consequence: a credential in argv also lands in shell history.
- No hardcoded absolute paths to a specific workspace.

Per `system-knowhow/best-practices.md`:

- Script lives with its sole consumer (single-app script in `apps/<id>/scripts/`, not shared `data/scripts/`).

Per `system-knowhow/best-practices.md` § "The workspace database: `LUCIDOS_DATABASE_URL`":

- **Database access that leans on the engine's old `PG*` defaults.** The engine used to set `PGHOST`, `PGUSER`, `PGPASSWORD`, `PGPORT` and `PGDATABASE` in every subprocess, and its own `DATABASE_URL` leaked into many. It sets neither now (ADR 0481). So a `psql`, `pg_dump`, `psycopg.connect()` or `pg.Client()` with no connection argument now reaches no database. Code reading `PG*` or `DATABASE_URL` for the workspace database finds nothing. Judge each `db-defaults` hit:
  - No connection argument, or a `PG*` / `DATABASE_URL` read meant for the workspace database: severity **broken**.
  - A write through `$LUCIDOS_DATABASE_URL` with no `BEGIN READ WRITE`: severity **broken**, since the handle is read-only by default.
  - Skip a call that already passes `$LUCIDOS_DATABASE_URL` and only reads. Skip one aimed at another database the script configures itself, such as a project `DATABASE_URL` the user set as an environment variable (`lucidos env-vars list`).
  - A hit on a wrapper (`def psql(`, `function psql(`): read the wrapper body. The connection argument lives there or is missing there. Calls to the wrapper are noise.

  This is the ADR 0214 migration check for ADR 0481.

## Remediation

Fixes run only on request, as the root's § Remediation says. A fix gets the
table for its finding, and the root's § Inline or a coding agent picks its
route.

### Pointing database access at `LUCIDOS_DATABASE_URL`

**Route: inline**, except the last row. Each of the others passes the same
variable to an existing call. Wrapping a write goes to a coding agent, since
the user should read each write before Apply.

| Old | New |
|---|---|
| `psql -At -c "…"`, any flags | `psql "$LUCIDOS_DATABASE_URL" -At -c "…"` |
| `pg_dump …` | `pg_dump --dbname="$LUCIDOS_DATABASE_URL" …` |
| `psycopg.connect()` or `psycopg2.connect("")` | `psycopg.connect(os.environ["LUCIDOS_DATABASE_URL"])` |
| `new pg.Client()` | `new pg.Client({ connectionString: process.env.LUCIDOS_DATABASE_URL })` |
| `$DATABASE_URL`, `os.environ["DATABASE_URL"]` or a `PG*` read, meant for the workspace | the same read of `LUCIDOS_DATABASE_URL` |
| A write: `INSERT`, `UPDATE`, `DELETE` or DDL | the same statements inside `BEGIN READ WRITE; … COMMIT;` |

Three rules the rewrite has to follow:

- **Name the variable, never its value.** A pasted URL puts the password in the script and in every tool call that runs it.
- **List every write you wrapped** in the report's `## Remediation` section. The read-only default exists to catch an accident, so a human confirms each write was meant.
- **Leave a project's own database alone.** A connection the script configures for another database is not a finding.
