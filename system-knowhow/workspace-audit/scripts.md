---
name: Workspace audit: scripts
description: The scripts audit section: CLI usage for data writes, events and API calls, hardcoded machine paths, and database access through LUCIDOS_DATABASE_URL.
sources: ["system-knowhow/lucidos-cli.md", "system-knowhow/lucidos-cli/*.md", "system-knowhow/best-practices.md", "system-knowhow/best-practices/*.md", "system-knowhow/plugins.md", "system-knowhow/plugins/*.md", "crates/lucidos-cli/src/data.rs", "crates/lucidos-core/src/core/agent_role.rs"]
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

- **An arming report read only through its first page.** A `lucidos await-event`
  script can receive `pending_cards.next_after`. Check that it follows the
  continuation with `lucidos event-waits pending-cards` before acting on the set.
- **A handed-off Apply retried as a failed push.** Treat a queued response as pending and follow its outcome through the source thread.
- **Plan marker scripts expecting thread evidence from a terminal.** A CLI call outside a coding-agent thread writes the marker alone.
  See `system-knowhow/lucidos-cli/subcommands-11.md` § `lucidos planned mark`.

- **Apply scripts mistaking a held build for failure.** Check result handling against `system-knowhow/lucidos-cli/subcommands-10.md` § `lucidos changes apply <change-id>`.
  Flag automatic retries based solely on a waiting version build as **broken**.
- **Data writers relying on Apply status.** Verify delivery from the data command result.


- **Routed scripts moving source credentials.** Keep inherited source-engine configuration when using `--via` and `--to`.
  Check `system-knowhow/lucidos-cli/waiting-through-an-engine-restart.md` § `lucidos workspaces targets`.
  Flag scripts copying origin tokens to another engine or automatically retrying uncertain routed spawns as **broken**.

- **Update scripts resetting saved addresses.** Check channel commands against `system-knowhow/lucidos-cli/subcommands-14.md` § `lucidos update-channel`.
  Flag scripts deleting custom slots merely to switch to Stable as **broken**; `set stable` preserves them.
  Flag scripts treating exit 3 as success or retrying before the owner's Allow once as **broken**.

- **Retrying a spawn that awaits permission.** The CLI explains direct fallback on stderr, including a generic note when gateway routing cannot carry the workspace name.
  Follow `system-knowhow/lucidos-cli/subcommands-4.md` § `lucidos spawn-thread`; waiting for Allow is not a failed delivery.
  A routed spawn whose `remote_start` names the target owner's card is waiting too. Flag scripts that spawn again on it as **broken**.

- **Trigger scripts working around a grant card.** Check trigger writes against `system-knowhow/lucidos-cli/subcommands-5.md` § `lucidos ask-permission <action> --trigger <id> --reason <R>`.
  Flag scripts that pass relative or home-folder grant paths, or rerun a trigger before the owner answers, as **broken**.

- **Raw host reads substituted for transfers.** Flag scripts bypassing the owner card or reading protected host captures as **broken**.
  Use `system-knowhow/lucidos-cli/subcommands-2.md` § `lucidos diagnostic-log prepare-host-transfer`; inherited confinement remains enforced.

- **Scripts forcing input after every ceiling handoff.** Check successor handling against `system-knowhow/lucidos-cli/subcommands-12.md` § `lucidos successor start`.
  Read `starts`: an interrupted narrative uses `after-this-turn`; a natural turn end or engine brief uses `on-next-input`.
  Flag scripts sending Continue without checking the recorded start as **stale**.

- **Hold scripts limited to Home descendants.** Check targets against `system-knowhow/lucidos-cli/subcommands-3.md` § `lucidos threads hold --reason <why> [--thread <uuid>]`.
  Home can hold any thread in its workspace; other agents retain subtree reach.

- **Scripts treating a ladder refusal as failure.** Check ladder edits against `system-knowhow/lucidos-cli/subcommands-4.md` § Editing a ladder.
  Exit 2 with a request id means the owner has not allowed the edit yet; ask with the id, then run it again.
  Flag scripts that retry on a timer or write `intelligence_ladder_*` preferences directly as **broken**.

- **Plan approval inferred from marker commands alone.** Calling-thread planning commands record lifecycle facts atomically with marker transitions.
  For reused approval, follow `inherited_from_event_id` to the original decision.
  Flag scripts that treat a parent's message as implementation approval for a child.
  Check inherited approval against `system-knowhow/lucidos-cli/subcommands-11.md` § `lucidos planned mark`.
  Do not reconstruct approval from plan prose or replay old events to recreate edit permission.

- **Workspace deletion repeated after batch approval.** Check `system-knowhow/lucidos-cli/waiting-through-an-engine-restart.md` § `lucidos workspaces delete`.
  The batch form runs on Allow once. Use the batch receipt to tell which workspaces moved to Trash.

- **Device sessions in script output.** Check rig scripts against `system-knowhow/lucidos-cli/waiting-through-an-engine-restart.md` § `lucidos workspaces device-session`.
  Flag printing the session or passing it through argv as **broken**; use the private output file and browser-context headers.
  Lasting consent must retain the exact device, TTL and gateway-recorded creator scope.
  Apply `system-knowhow/workspace-audit/cross-cutting`'s direct-port device-authority check to session minting and retries.

- **Parsing thread-scoped event output.** Parse CLI stdout as JSON and treat its stderr liveness header as current metadata.
  See `system-knowhow/lucidos-cli/subcommands.md` § `lucidos events query`; HTTP event response shapes remain unchanged.

- **Scripts calling a gateway over plain loopback HTTP.** A gateway's loopback answers https only, with the local certificate.
  Plain `http://localhost:<gateway port>` gets a `301` or the hand-off page, never the API.
  Flag a gateway call over `http://` on loopback, or one that passes `curl -k`, as **broken**.
  Recommend the `lucidos` CLI, which trusts the local certificate itself; see `system-knowhow/remote-access/the-three-routes.md` § The local certificate.
  Skip calls to `$LUCIDOS_API_BASE_URL`: that is the engine, not the gateway.

- **Scripts overriding repair build priority.** Repair descendants inherit `LUCIDOS_BUILD_SLOT_CLASS` from durable repair provenance.
  Check build wrappers against `system-knowhow/lucidos-cli/subcommands-6.md` § `lucidos build-slot`.
  Flag scripts upgrading automatic repairs to priority work as **broken**.

- **Scripts counting build-slot queue time against the task watchdog.**
  Check background job timeouts against `system-knowhow/lucidos-cli/subcommands-6.md` § `lucidos background-task run`.

- **Scripts treating a refused lease as permission to build.** The source launch wrapper distinguishes `unleased` from a wait timeout.
  Flag scripts that use the hidden `lucidos build-slot --lease` mode and continue after exit 75 as **broken**.

- **Draft scripts reconstructing snapshots from separate reads.** A single-draft read returns text, ordered image hashes, settings, revision and epoch together.
  Check consumers against `system-knowhow/lucidos-cli/subcommands-2.md` § `lucidos threads drafts`; lists omit the snapshot.
  Flag scripts combining a list preview with separately fetched settings as **stale**; those fields need not describe the same saved revision.
  Recommend the documented single-draft read. Do not submit, clear or alter drafts during this audit.

- **Draft creation retried without a key.** Check scripts against `system-knowhow/lucidos-cli/subcommands-3.md` § `lucidos threads create-draft`.
  Flag a script that retries a call without passing the same `--retry-key` as **broken**: each retry hands the owner another draft.

- **Draft submission retried with a fresh key.** Check scripts against `system-knowhow/lucidos-cli/subcommands-2.md` § `lucidos threads submit-draft`.
  Flag a script that retries `threads submit-draft` with a new `--retry-key` as **broken**: a fresh key is a new send, not a receipt read.

Check script recipes that expect automatic memory recall of unattended trigger output.
Those writes remain readable files; see `docs/system-events.md` § Artifact provenance and provider holds.

- **Harness cleanup.** Check cleanup recipes against the scoped owner-token contract in `docs/dev-runtime.md`. An eval prefix never authorizes filesystem or database deletion.

- Snapshot automation follows `system-knowhow/lucidos-cli/subcommands-5.md` § `lucidos snapshot`.
  Flag a script that repeats restore after a lost response as **broken**.
  Recommend the documented restore-status command.

- **Scripts restoring a backup in one request.** A backup restores in the workspace picker; see `system-knowhow/backups.md` § Restore.
  Flag a script that posts an archive to the gateway's `workspaces/restore` endpoint as **broken**: that endpoint takes only a finished upload session (ADR 0810).

For each finding, capture: **location**, **what's wrong**, **which reference owns the rule** (link, don't quote), **suggested fix**.

Per `system-knowhow/lucidos-cli.md`:

- Writes to `data/` go through `lucidos data write`, not raw HTTP and not open-coded paths under `$LUCIDOS_WORKSPACE/data/`. Do not flag a script's own runtime state (a cursor, a last-seen id) written directly under `data/artifacts/<plugin-id>/` or `data/triggers/<slug>/state/`. That is the intended pattern, per `system-knowhow/plugins/authoring-a-marketplace.md` § "Where a plugin keeps its runtime state".
- Flag a script that automatically repeats a data write after a CLI timeout.
  Check `system-knowhow/lucidos-cli/subcommands.md` § `lucidos data write`.
  Compare any restart retry logic with `system-knowhow/lucidos-cli/waiting-through-an-engine-restart.md` § Waiting through an engine restart.
- Domain events go through `lucidos events emit` / `lucidos events query`.
- External API calls go through `lucidos proxy <name>` when the workspace owns a credential for the service. The patterns are the `cross-cutting` section's. A script adds one consequence: a credential in argv also lands in shell history.
- No hardcoded absolute paths to a specific workspace.

Per `system-knowhow/best-practices.md`:

- Script lives with its sole consumer (single-app script in `apps/<id>/scripts/`, not shared `data/scripts/`).

Per `system-knowhow/best-practices/workspace-layout.md` § "The workspace database: `LUCIDOS_DATABASE_URL`":

- **SQL writes to the workspace database.** The agent role writes no table, even inside `BEGIN READ WRITE`.
  Flag each `INSERT`, `UPDATE`, `DELETE` or `TRUNCATE` sent through `$LUCIDOS_DATABASE_URL` as **broken**, and name the command from the reference's table.
  A script that pipes several statements with no `ON_ERROR_STOP` exits zero after the refusal, so say that it fails in silence.
  In a machine-made workspace the write still works. Do not call it broken there: say that it announces nothing, so an open page shows the old data.
  Never recommend broader grants or an origin change to bypass the refusal.

- **Database access that leans on the engine's old `PG*` defaults.** The engine used to set `PGHOST`, `PGUSER`, `PGPASSWORD`, `PGPORT` and `PGDATABASE` in every subprocess, and its own `DATABASE_URL` leaked into many. It sets neither now (ADR 0481). So a `psql`, `pg_dump`, `psycopg.connect()` or `pg.Client()` with no connection argument now reaches no database. Code reading `PG*` or `DATABASE_URL` for the workspace database finds nothing. Judge each `db-defaults` hit:
  - No connection argument, or a `PG*` / `DATABASE_URL` read meant for the workspace database: severity **broken**.
  - A write through `$LUCIDOS_DATABASE_URL`: severity **broken**, under the check above.
  - Skip a call that already passes `$LUCIDOS_DATABASE_URL` and only reads. Skip one aimed at another database the script configures itself, such as a project `DATABASE_URL` the user set as an environment variable (`lucidos env-vars list`).
  - A hit on a wrapper (`def psql(`, `function psql(`): read the wrapper body. The connection argument lives there or is missing there. Calls to the wrapper are noise.

  This is the ADR 0214 migration check for ADR 0481.

## Remediation

Fixes run only on request, as the root's § Remediation says. A fix gets the
table for its finding, and the root's § Inline or a coding agent picks its
route.

### Pointing database access at `LUCIDOS_DATABASE_URL`

**Route: inline**, except the last two rows. Each of the others passes the same
variable to an existing call. Changing a write goes to a coding agent, since
the user should read each write before Apply.

| Old | New |
|---|---|
| `psql -At -c "…"`, any flags | `psql "$LUCIDOS_DATABASE_URL" -At -c "…"` |
| `pg_dump …` | `pg_dump --dbname="$LUCIDOS_DATABASE_URL" …` |
| `psycopg.connect()` or `psycopg2.connect("")` | `psycopg.connect(os.environ["LUCIDOS_DATABASE_URL"])` |
| `new pg.Client()` | `new pg.Client({ connectionString: process.env.LUCIDOS_DATABASE_URL })` |
| `$DATABASE_URL`, `os.environ["DATABASE_URL"]` or a `PG*` read, meant for the workspace | the same read of `LUCIDOS_DATABASE_URL` |
| A SQL write to `preferences`, `notification_reads`, `pinned_apps`, `memory_entries` or `browser_logins` | the `lucidos` command the reference's table names for it |
| A SQL write to any other table | the relevant CLI or API operation |

Three rules the rewrite has to follow:

- **Name the variable, never its value.** A pasted URL puts the password in the script and in every tool call that runs it.
- **List every write you wrapped** in the report's `## Remediation` section. The read-only default exists to catch an accident, so a human confirms each write was meant.
- **Leave a project's own database alone.** A connection the script configures for another database is not a finding.

## Diagnostic evidence

Run `lucidos diagnostic-log query` to investigate script failures that appear in Lucidos logs.
See [Agent Sandbox](../agent-sandbox.md) for the access rules.
A maintainer bundle omits legacy log text; its absence does not establish an audit finding.

- **Agent-started frontend preview processes.** Use `lucidos frontend-preview start` so the engine supervises the process beyond the agent turn.
  Flag procedures starting Vite directly as **broken** when they promise a preview that outlives that turn.
  Reference: `system-knowhow/lucidos-cli/subcommands-12.md` § `lucidos frontend-preview`.
