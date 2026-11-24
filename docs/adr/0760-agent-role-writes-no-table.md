# 0760: The agent role writes no table in a user workspace: each state change goes through the command that announces it

- **Status**: Accepted
- **Date**: 2026-11-21
- **Supersedes**: the seven-table write list in the first decision paragraph of
  [0686](0686-agent-role-table-write-allow-list.md), for user and unknown origins.
- **Amends**: [0481](0481-engine-db-credentials-stay-out-of-subprocesses.md)
  (who may write inside `BEGIN READ WRITE`) and
  [0756](0756-gateway-subscribes-to-engine-events.md) (its last consequence).

## Context

ADR 0686 let the agent role write seven tables by SQL: `preferences`,
`memory_entries`, `summary_tree_nodes`, `summary_tree_scopes`,
`notification_reads`, `pinned_apps` and `browser_logins`.

A direct write emits no event. Clients follow events, so every open page kept
the old data until a reload. ADR 0756 records one case, the picker badge after
a write to `notification_reads`.

## Decision

In a workspace whose origin is not machine-made, the agent role holds no write
privilege on any table, view or sequence in schema `public`.

- `AGENT_WRITABLE_TABLES` is deleted. Every table gets `SELECT`, minus the
  hidden columns, as before.
- Sequences get `SELECT` only. `nextval` is a write, so `USAGE` goes only to a
  role that inserts.
- `provision` ends with a privilege sweep. It fails the boot and names the
  relations when the role can still write one, through any grant path.
- Each change an agent made by SQL has a `lucidos` command that announces it.
  `system-knowhow/best-practices.md` holds the table of them.

A machine-made workspace (`agent`, `demo`, `eval`, `e2e`) keeps the grants ADR
0686 gave it. The sweep does not run there.

## Rationale

A state change announces by default. A grant is the one path that cannot
announce, so the fix is to remove the path, not to patch each stale reader.

The sweep exists because `REVOKE ... FROM <role>` cannot remove a grant to
`PUBLIC` or to a group. A revoke alone would leave such a write open in
silence. A failed boot is loud, and it names the relation to fix.

The refusal is a plain Postgres privilege error. Postgres cannot attach a hint
to it, so knowhow, the workspace audit and a release notice name the commands.

## Consequences

- Every workspace on every install shape converges at its next engine boot,
  because `grant_tables` revokes before it grants.
- A user script or trigger that wrote one of the seven by SQL now fails with
  `permission denied for table <name>`. We cannot scan for those. The release
  notice `agent-database-writes-closed` shows to every workspace and offers the
  audit.
- A script that pipes several statements with no `ON_ERROR_STOP` exits zero and
  stops writing in silence. The audit flags it statically.
- ADR 0481's consequence "a script that writes must say so with
  `BEGIN READ WRITE`" now holds only in a machine-made workspace.
- ADR 0756's last consequence is closed for user workspaces. A machine-made
  workspace's direct write still announces nothing.
- The two summary tree tables get no command. The engine rebuilds both from
  events, so a direct edit never lasted.
- A direct `INSERT` into `memory_entries` has no like-for-like command. A
  memory rebuild deleted such a row. `memory correct-by-id` records a
  correction, which the rebuild replays.
- The new commands are CLI only. SQL was only reachable from a shell, and a new
  tool schema bills every turn.
- Provisioning revokes `CREATE` on schema `public` from PUBLIC and from the
  role, for every origin. A database made before Postgres 15 gave PUBLIC that
  right, and a role that creates a table owns it. Provisioning fails the boot
  when the role can still create there, and the error names the revoke to run.
  That boot failure reaches one install shape: a database made before
  Postgres 15, with an engine role that is no superuser and does not own the
  schema. The bundled and Docker installs connect as a superuser.
- Unchanged: the read-only default, secret columns, `READ_ONLY_TABLES`, the
  human-only preference trigger, and the role's `TEMPORARY` on the database.
  This is still a grant boundary, not a SQL sandbox.
- The sweep leaves out a relation the role owns, and every materialized view.
  A table the role made before the revoke stays the role's own, and it can
  still write it and grant on it. The owner decided to leave it so. A sweep
  that counted it would let an agent stop its own workspace booting.
- Left open by the owner's decision, each named in the deviations file beside
  the plan: the silent multi-statement script, the notification reader gap,
  the `memory` tool recording no caller, and the locale reset race.
- Handing the role's relations to the engine's role was tried and backed out.
  A view, a `SECURITY DEFINER` function and an expression index all run as
  their owner. Owned by the engine, each would run an agent's SQL with the
  engine's rights.

## Alternatives considered

- **Close machine-made workspaces too.** Tour production backdates events by
  SQL, and that needs a seed route first. The owner chose to keep the rule.
- **A refusal trigger on the seven, naming the command.** The role would keep
  the privilege, so the boundary would rest on a trigger being present. The
  owner chose the plain revoke.
- **Revoke with no sweep.** A grant through `PUBLIC` or a group survives it,
  and nothing would report that.
