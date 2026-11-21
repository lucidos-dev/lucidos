# 0686: Agent database writes use an origin-aware table allow-list

- **Status**: Accepted
- **Date**: 2026-11-14
- **Amends**: [0481](0481-engine-db-credentials-stay-out-of-subprocesses.md)
  and [0509](0509-stored-secrets-are-sealed-at-rest.md): agent-role write grants.
- **Supersedes**: the read-only table list in decisions 8 and 9 of
  [0612](0612-local-workspace-snapshots.md), for user and unknown origins.

## Context

A read-only default does not fence authority. `BEGIN READ WRITE` lifted it,
and the agent role could forge events, approvals, device registrations and
thread ancestry through direct SQL. API caller checks cannot constrain those
writes. New tables inherited broad write grants at the next boot.

## Decision

The agent role writes only `preferences`, `memory_entries`,
`summary_tree_nodes`, `summary_tree_scopes`, `notification_reads`, `pinned_apps`
and `browser_logins`. Human-only preference keys retain their trigger guard.
All other tables receive SELECT only. Secret columns remain inaccessible.

Machine-made workspaces retain broader write grants. The gateway passes its
registered origin as `LUCIDOS_WORKSPACE_ORIGIN`; `agent`, `demo`, `eval` and
`e2e` select that policy. Missing, unreadable, `user`, `unknown` and unfamiliar
values select the allow-list. The variable is reserved against stored rows.

The four always-read-only tables remain `_sqlx_migrations`, `backup_recipient`,
`email_accounts` and `snapshot_restore_boundary`. Tables holding secrets still
receive only column-level reads, in every origin.

## Rationale

An allow-list fails closed when a migration adds an authority table.
Agents can still shape content and UI state without forging owner consent.
Machine-made workspaces belong to agent workflows, including tour production
that deliberately backdates events through SQL.

## Consequences

All install shapes receive the grants at engine boot. Provisioning revokes
existing grants before rebuilding them, so existing workspaces converge too.
An origin reclassification takes effect when the engine next starts.
Standalone engines with no vouched origin use the allow-list.

Direct SQL writes outside the list fail loudly. Use the relevant CLI or API
for those operations. Schema creation rights and the read-only transaction
default are unchanged. This is a table-grant boundary, not a SQL sandbox.

Tests cover every migrated table, runtime-created memory tables, fresh tables,
stale grants, origin transitions and reserved environment names.

## Alternatives considered

- Extend the deny-list: every new authority table would reopen the hole.
- Restrict all origins: agent-owned workspace workflows need deliberate writes.
- Add an owner switch per workspace: rejected in favor of registered origin.
