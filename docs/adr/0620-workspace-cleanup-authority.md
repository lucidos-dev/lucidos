# 0620: Only a paired device deletes user workspaces; harness cleanup stays owner-scoped

- **Status**: Accepted
- **Date**: 2026-11-03

## Context

Workspace folders, databases and archived coding-agent worktrees accumulate on
both source and packaged installations. Cleanup is destructive across independent
resources: a directory name alone proves neither database ownership nor user
consent. A machine-local credential is available to coding agents and harnesses,
so possession of it cannot stand for a person's decision to delete their data.

## Decision

Only a paired device may delete a workspace whose origin is `user` or
`unknown`, confirm a frozen cleanup batch, or empty Trash. Protected workspaces
require individual named confirmation. A harness may delete only an ephemeral
workspace using the owner token minted for that registration; the local token
alone grants no deletion authority.

Review freezes exact resources and observations. Confirmation revalidates them,
refusing changed or unknown ownership. Directory and database ownership are
independent, and all cooperating gateways serialize resource publication against
cleanup. An unreadable peer registry prevents orphan classification.

Deletion stops engines, takes the engine lock, preserves recoverable state in
Trash, moves the folder and then drops the database. Protected data is dumped
before moving or dropping it. Durable journals and actor-attributed audit records
make interruption repair explicit. Restore does not launch an engine, and
worktree cleanup archives Git objects without deleting branches.

## Rationale

The authority follows the resource's provenance, rather than a naming convention
or the machine on which a caller runs. Legacy registrations remain protected as
`unknown`. Expiry is a review hint, never an automatic deletion schedule. Frozen
observations prevent a review from authorizing a later, changed resource.

Harness credentials are scoped to a registration generation. Eval pins the
original credential or its absence before driving each arm; retries must never
borrow a later registration's token. Durable scoring snapshots precede cleanup,
so removing disposable runtime resources does not remove the evidence.

## Consequences

Source and packaged gateways on macOS and Linux, running as the same OS user,
participate in ownership discovery. Legacy engines without a gateway are outside
that inventory and must not be inferred safe from an absent registration.

Cleanup requires explicit review or harness ownership. Unknown observations and
partial failures retain resources and report the problem. Trash remains until a
paired device empties it; no timer deletes data. The implementation plan is
`docs/plans/2026-11-02-workspace-cleanup.md`.

## Alternatives considered

- Delete by prefix or age: neither proves ownership or consent.
- Give the local token device authority: every coding-agent subprocess would
  inherit permission to delete user data.
- Scan only the current registry: another gateway may own the same database.
- Delete directly from harness filesystem/database code: bypasses shared locks,
  authorization, recoverability and audit.
- Automatically expire Trash: turns a review hint into an unconfirmed deletion.
