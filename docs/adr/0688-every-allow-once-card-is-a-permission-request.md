# 0688: Every Allow once card is a permission request

- **Status**: Accepted
- **Date**: 2026-11-14
- **Amends**: ADR 0387, ADR 0647, ADR 0651, ADR 0664, ADR 0671, ADR 0676

## Context

A Demo Director film take creates one workspace per take. Each create asked a
card that offered only Allow once, so a film of ten takes cost ten taps. The
owner wanted the regular scopes there: once, for this thread, always.

ADR 0664 had already settled one permission model. Only the automation pause
and batches used it. Every other engine card still ran on the single-use path
of ADR 0387: thread verbs outside a subtree, the workspace broker, snapshot
restore, a saved-draft send and an unattested caller's act. That path had its
own events, its own spend lookup, its own CLI command and its own name.

## Decision

**Every engine card that offered only Allow once is now a permission request.**

- The engine words each card from its action's **menu**, one pure function
  (`engine::permission::menu`). No gate words its own options.
- The owner's pick records `PermissionGranted`. A gate asks `live_permission`
  and spends a once grant with `spend`. Nothing reads `OwnerApprovalSpent` or
  `UnattestedActSpent` any more.
- An option carries its own breadth. Allow once covers the asked target.
  Allow for this thread and Always allow may cover any target of the action.

| Action | Allow once | Allow for this thread | Always allow |
|---|---|---|---|
| create-workspace | the asked slug | any slug | any slug |
| stop, start, set-autostart, rename | that workspace | that workspace | that workspace |
| archive, cancel, continue, control, set-aside, bring-back, side questions | that thread | that thread | that thread |
| apply, revert | that thread | that thread | no |
| discard, submit-draft, delete-workspace, restore-workspace, restore-snapshot | the exact target | no | no |
| an unattested caller's act or start | that exact request | no | no |

Every card also offers Don't allow.

- Allow for this thread holds on the asking thread. Always allow holds on the
  workspace.
- A once grant ends at its holder thread's next turn start, as ADR 0387 had.
- A snapshot restore ends every grant from before it.
- An agent asks with `lucidos ask-permission` (`POST
  /api/v1/permission-requests`), which records `PermissionRequestPrepared`. It
  then asks its question tool with the printed id, and the engine swaps in the
  card. A held grant answers 409, and the agent acts with no card.
- The broker's refusal slug is `permission_required`. The snapshot restore's
  is `snapshot_restore_permission_required`.
- A draft send spends its grant inside the transaction that accepts the draft
  (`DraftSubmissionAuthority::Permission`).
- A batch picks only Allow once or Allow for this thread, never Always allow,
  and never answers an unattested caller's card.

**The ADR 0387 name for the card is a retired term** (`docs/retired-terms.tsv`).
The CLI keeps `ask-owner-approval` as a hidden alias for one release
(`docs/temporary-measures.md`).

## Rationale

One model means one set of rules for who may answer, how long an answer lasts
and what it covers. The single-use path duplicated every one of them. It also
could not offer a lasting scope, which is what the film rig needed.

The menu keeps the judgment in one reviewable place. Acts that destroy work or
send the owner's words stay once-only for everyone. A delete, a Trash restore
and a snapshot restore cannot be undone by the next card. ADR 0651 binds a
draft send to one exact text, so a lasting grant would send words the owner
never read.

## Consequences

- One Always allow tap on a create covers every later take. `lucidos
  permissions list|revoke` shows and ends it.
- `OwnerApprovalRequested`, `OwnerApprovalSpent`, `UnattestedActSpent` and the
  `owner_approval` and `unattested_act` question fields stay in the enum, so
  stored rows read. Nothing emits them. A test gate refuses a new emit site.
- A card raised by the old path and still unanswered at the switch grants
  nothing. Its caller asks again.
- A batch raised before the switch and still unconfirmed skips its card items:
  the card digest now hashes the permission request.
- ADR 0676 is unchanged in effect: an agent acts on its own machine-made
  workspace with no card. Create gets no no-card rule of its own.

## Alternatives considered

- **A no-card rule for an agent's create.** Rejected: it patched the one card
  that hurt and left every other single-use card in place. One Always allow tap
  does the same job and stays revocable.
- **Keep the single-use path beside permissions.** Rejected: two models with
  different rules for the same question is the drift ADR 0664 set out to end.
- **Offer Always allow on every action.** Rejected for the once-only rows
  above: those acts destroy work or send the owner's words.
