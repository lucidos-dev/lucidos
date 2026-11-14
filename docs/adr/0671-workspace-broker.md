# 0671: The workspace broker: agents control workspaces through the engine, per-call owner approval, no restart

- **Status**: Accepted
- **Date**: 2026-11-13
- **Amends**: [0664: One permission model](0664-permissions-and-the-pause-family.md) (its action list and batch items)
- **Amended by**: [0676: Agent-made workspaces are the agent's](0676-agent-made-workspaces-are-the-agents.md) (stop, start, autostart and rename on a machine-made workspace need no card); [0688: Every Allow once card is a permission request](0688-every-allow-once-card-is-a-permission-request.md) (a broker card is a permission request with its action's menu)

## Context

The gateway control plane (`/~/api/v1/control/...`) needs the machine-local
token. ADR 0664 makes a secret read non-grantable, so an agent cannot reach
it. A demo rig still has to stop, start, create, rename and toggle autostart
on fixture workspaces, and to clean them up after.

Two facts shaped the design:

- A gateway restart can re-exec the gateway when its binary is stale. One
  restart call once started 37 engines.
- The registry could not say who made a workspace in a way an agent may rely
  on. Every registration before this change reads `unknown`.

## Decision

An agent controls workspaces through its own engine, which holds the token and
calls the gateway. `lucidos workspaces` carries the verbs: `create`, `stop`,
`start`, `autostart`, `rename`, `delete`, `restore` (from the Trash) and
`classify`. The routes live under `/api/v1/workspaces`.

- **One `Action` per verb**: `CreateWorkspace`, `StopWorkspace`,
  `StartWorkspace`, `SetWorkspaceAutostart`, `RenameWorkspace`,
  `DeleteWorkspace`, `RestoreWorkspace`. This amends ADR 0664's action list.
- **Per-call owner approval.** Each mutating call spends one Allow once for
  exactly that verb and that workspace (ADR 0387). With none, the engine
  records the request and answers 403 `owner_approval_required` with its id.
  The agent asks with it, then runs the same call again.
- **Agents only.** A caller with no thread-bound origin token is refused. The
  owner keeps the picker, unchanged.
- **The verb split.** Create, stop, start, autostart and rename take any
  workspace. Delete and restore take a *machine-made workspace* only: origin
  `agent`, `demo`, `eval` or `e2e`. The gateway enforces it under the
  workspace lock, from `WorkspaceOrigin::is_machine_made`. The engine checks the
  gateway's own `machine_made` flag first, so no card asks for a doomed act.
- **No restart, anywhere.** No broker route, gateway broker route or CLI verb
  restarts. `POST /api/v1/workspaces/:id/restart` answers 405: stop, then
  start.
- **Create names its slug** and refuses a taken slug or name, never suffixing
  it, so the approval names the workspace that gets made. It stamps origin
  `agent`, ephemeral, and `created_by` (workspace and thread).
- **Autostart off for agent-made workspaces** unless the call asks for it, so a
  rig cannot fill the next boot with engines.
- **Classify** is a batch item, `workspace_classification` (ADR 0664's batch).
  One card line per proposed origin, with a count and the agent's evidence,
  quoted. Only the owner's Run all moves an entry, and only an `unknown` one.
- **Snapshot restore narrows.** An agent can no longer restore a snapshot into
  a `user` or `unknown` workspace, its own included. The gateway refuses with
  `snapshot_restore_target_protected` on the caller-verification call.

## Rationale

- **The token stays in one process.** Moving the call into the engine keeps
  ADR 0664's rule that a secret read is never granted. The engine already sends
  the token on every gateway call, as a sensitive header. The broker passes
  back only the gateway's `error` text, with the token redacted.
- **Per call, not standing.** A standing broker permission would let one yes
  cover every later stop and delete. The rig acts rarely, and each act is
  visible, so a card per act costs little and keeps the owner in the loop.
- **Delete is where harm lives.** Stopping or renaming a user's workspace is
  reversible and loud. Deleting it is not, so only harness-made workspaces
  qualify. A name or folder prefix never decides it: only the registry entry.
- **No restart** because stop then start does the same job for one engine,
  and cannot re-exec the gateway.
- **The snapshot gate sits on verification**, not on the approval spend.
  Under the `snapshot_restore_by_agents` policy `allow`, a restore never
  reaches the approval spend. A check placed there would have skipped exactly
  the case with no card. The caller-verification call runs under both
  policies, so the gateway checks the target there.

## Consequences

- The Demo Director rig can drive fixture workspaces from an agent thread, one
  approval per act.
- Existing workspaces stay `unknown` and protected until the owner confirms a
  classify batch.
- A Trash restore names the Trash entry id, which the agent reads from
  `lucidos workspaces trash`. Deleting a machine-made workspace keeps no
  database copy, as before this change, so restore refuses such an entry until
  delete keeps one.
- The picker's cleanup review treats origin `agent` like the other
  machine-made origins: a filter pill, and stale agent-made workspaces
  suggested for cleanup.
- A classification that a refusal stops midway records `partly_applied`, never
  `skipped`, since some origins already moved.
- A trigger run cannot use the broker: no owner answers inside its turn.
- A no-gateway engine answers 503: there is no registry.
- The reliability work owns permission storage. This change adds only `Action`
  variants and one batch item kind, and touches no grant store.

## Alternatives considered

- **Hand the agent the local token, gated by a card.** Rejected: ADR 0664 makes
  a secret read non-grantable, and the token would sit in the agent's
  transcript and environment.
- **A standing "workspace control" permission.** Rejected by the owner in
  favour of per call: one yes would cover deletes the owner never saw.
- **Infer machine-made from a name prefix** (`demo-`, `e2e-`). Rejected: a
  user can name a workspace anything, and a wrong guess deletes their work.
- **Keep a restart verb that only restarts the engine.** Rejected: the gateway
  restart path is the one that re-execs, and two near-identical verbs invite
  the wrong one. Stop then start is explicit.
- **Suffix a taken slug on create**, as the picker does. Rejected: the
  approval would name a workspace other than the one made.
