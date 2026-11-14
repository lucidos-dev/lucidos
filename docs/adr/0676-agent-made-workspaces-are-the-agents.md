# 0676: An agent stops, starts, renames and sets autostart on a machine-made workspace with no card; every other workspace asks the user each call

- **Status**: Accepted
- **Date**: 2026-11-13
- **Amends**: [0671: The workspace broker](0671-workspace-broker.md) (its per-call card)

## Context

ADR 0671 made every broker call ask the user, even on a workspace an agent
made itself. A demo rig stops and starts its fixture workspace between takes,
and each stop and each start raised a card. The user answered the same card
many times for workspaces nobody but the rig uses.

## Decision

**Whoever made a workspace decides for it.**

- **A machine-made workspace is the agent's.** Stop, start, autostart and
  rename on one act at once, with no card. Machine-made is the gateway
  listing's `machine_made` flag, the same flag that gates delete.
- **Every other workspace asks the user each call**, as in ADR 0671. That
  covers a `user` workspace and an `unknown` one.
- **Create, delete and Trash restore always ask**, whoever made the workspace.
- **Each no-card call records `MachineMadeWorkspaceChanged`** on the caller's
  thread: the verb, the workspace and the agent's reason. The engine records it
  before it calls the gateway. If the record fails, the call is refused.

## Rationale

- **The card guards the user's work.** A rig's fixture workspace holds none,
  so a card there only interrupts the user.
- **The four verbs are reversible.** Stop undoes start, and a rename renames
  back. Delete is not reversible, and create adds a workspace the user never
  saw, so both keep the card.
- **One marker, already trusted.** Delete already relies on `machine_made`.
  Reusing it adds no second notion of "the agent's".
- **Record, then act**, so every act an agent took with no card stays
  attributable to a thread, even when the gateway call fails after.

## Consequences

- An agent cannot dodge the card by marking a user workspace machine-made.
  `classify` moves only an `unknown` entry, and only on the user's Run all.
  Lowering an existing entry's origin needs a paired device. The OS sandbox
  hides the gateway data dir, so a sandboxed agent cannot edit the registry or
  the Trash. A test pins that last fact.
- An agent the user runs unsandboxed could edit the registry. It could also
  read the local token, so the broker is not the boundary there.
- The engine reads the listing, then calls the gateway's ordinary control
  route, which does not check origin again. In that window an entry can only
  move from machine-made to user, which needs a paired device. The four verbs
  are reversible, so the window is accepted.
- The existing card for a user's workspace is unchanged: same wire slug, same
  request id, same CLI exit code.

## Amendment: snapshot restore joins the no-card verbs

An agent restores a snapshot into a machine-made workspace with no card. A
film rig restores its fixture for every alternate take. Each restore
spent an Allow once on a workspace that holds none of the user's work.

- **The source engine decides**, in the route that spent the card
  (`api::snapshots`). It reads the gateway listing, as the broker does, and
  asks the same `whose` question with the restore verb.
- **The restore is reversible.** It saves a verified recovery snapshot first
  and offers Undo restore (ADR 0612), so it sits with stop and rename, not
  with delete.
- **It records `MachineMadeWorkspaceChanged`** with the snapshot handle, before
  the source answers the target. A failed record refuses the restore. Under
  the target's `allow` policy the target never asks the source, so only the
  target records the restore, as it did before this amendment.
- **An unreadable listing refuses.** It never falls through to no card.
- **The gateway narrowing stays** (ADR 0671): an agent restore into a `user`
  or `unknown` workspace is still refused outright.
- In practice the `ask` value of `snapshot_restore_by_agents` no longer asks
  an agent, since the narrowing admits only machine-made targets. The
  preference stays for the owner's own policy record.

## Alternatives considered

- **Keep a card on every call.** Rejected by the user: the rig's cards bury
  the ones that matter.
- **A standing grant per workspace.** Rejected: it still asks once per
  workspace, and a grant can outlive the workspace it names. The origin already
  answers the question.
- **Also drop the card for delete on a machine-made workspace.** Rejected by
  the user: delete keeps no database copy, so a mistake cannot be undone.
