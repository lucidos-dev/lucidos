# 0560: A handoff starts a successor session in the same thread: supersedes 0474, 0503 and 0507, amends 0493, 0497, 0505, 0539 and 0557

- **Status**: Accepted
- **Date**: 2026-10-23
- **Supersedes**: [0474](0474-successor-threads.md), [0503](0503-build-chain.md), [0507](0507-successor-inherits-handoff-consent.md)
- **Amends**: [0557](0557-harden-runs-on-the-default-floor.md), [0493](0493-a-plan-cards-new-thread-pick-is-owner-consent.md), [0497](0497-plan-card-offers-a-new-thread-for-a-cheaper-implementer.md), [0505](0505-named-handoff-checkpoints.md), [0539](0539-successor-context-by-cache-warmth.md)

## Context

A heavy coding-agent thread handed off by spawning a *successor thread* (ADR
0474). The successor carried the branch and the plan marker, sat in the old
thread's place, and the old thread archived itself. Titles took a number.
Writing in the old thread raised a warning.

A child thread's successor needed authority under its parent. That grew the
*build chain* (ADR 0503) and the inherited *handoff consent* (ADR 0507). An
automatic checkpoint could still stall on an owner approval card.

Almost all of that existed only because the successor was a separate thread.
The work never needed a new thread. It needed a clean context.

## Decision

A handoff starts a *successor session*: a fresh Claude Code or Codex session in
the same thread, from a *handoff narrative* or a *handoff brief*. The thread
keeps its id, worktree, branch, plan marker, change and parent's wait. Nothing
creates a successor thread any more.

All four moments do this: a plan card's `Implement in a fresh session` pick,
"handoff this session", **Continue with context** on the *Successor card*, and
an automatic *handoff checkpoint*. **Start fresh** stays a plain new thread.

- **The record.** The new session's `SessionStarted` carries continuity
  `handoff`, with the checkpoint, its progress and the opening message. A resume
  that fails carries `resume-failed` and starts from a handoff brief, unless
  Apply or Discard ended the session on purpose.
- **The transcript.** The boundary draws the same *New session* event card as
  any fresh session, in state "Handed off". It names the checkpoint and the
  implementer, and folds the opening message under it. No bespoke divider.
- **The implementer.** A handoff may switch backend, model or effort. The
  switch happens only at the session boundary, so the rung lock (ADR 0542)
  holds. The thread's backend reads its newest session's.
- **Telemetry.** The session is a unit: every `ContextCaptured` names its
  session's ordinal. Context size, cache warmth and the *handoff estimate* read
  the current session only.
- **No authority.** Starting a session in its own thread is an act in the
  thread's own subtree. No owner approval card, no consent record.
- **A dirty worktree is fine.** The branch never moves, so nothing is lost. The
  opening message tells the session to read `git status` first.
- **The CLI.** `lucidos successor start` takes over from `spawn`, which stays
  an alias (a temporary measure). `offer` keeps its flags. Its "ask the owner"
  outcome is gone.

## Rationale

The successor thread solved a context problem with a topology change. Every
part of the topology then needed its own rule: placement, archiving, branch
carry, marker carry, title numbering, the stale-write warning, and authority
for children. Each rule was a place to break, and some did: ADR 0507 exists
because a consented chain stalled on a card one handoff later.

A session boundary inside the thread gives the clean context and nothing else.
The parent waits on the same thread, so it needs no consent. The branch never
moves, so no carry and no dirty-worktree refusal. One thread is one change, so
the old thread never has to stand aside.

The New session card is the card a fresh session already draws. Reusing it
keeps one concept, one component and one wording in the transcript.

## Consequences

- **Old chains still render.** Their events are immutable. "Continued in" and
  "Continued from" rows, the `successor_of` link and the `carried_by_successor`
  hold stay. A boot pass never proposes a legacy predecessor.
- **A legacy successor that never started leaves its old thread open.** The
  archive net is gone, and nothing new needs it.
- **`SuccessorThreadSpawned` and `HandoffConsent` stay as read-only legacy.**
  The `SpawnSuccessor` reach verb is retired like `CreateTopThread`, kept only
  so stored cards still parse.
- **The handoff estimate keeps its samples whole.** A sample runs from
  `HandoffCheckpointReached` to the first *clean turn end* in the same thread,
  across session boundaries. This amends ADR 0505, whose sample followed the
  successor thread.
- **The default floor needs no consent** (ADR 0557). A floor or risk rung
  is still named only to a thread below it, and the agent hands off onto it.
  The rung no longer stands as a consent, since a handoff needs none.
- **The plan card's option reads `Implement in a fresh session`.** ADR 0493's
  consent reading and ADR 0497's rule for a cheaper implementer still decide
  when the card offers it. Only the place changes, and no consent is needed.
- **The warm or cold choice stays the engine's, at the tap** (ADR 0539). A warm
  cache has the old session write the narrative, then end. A cold cache ends it
  at once and starts the new session from a handoff brief.
- **Thread cost** shows the thread's total. Per-session cost rows live in a
  workspace widget, not an engine API.

## Alternatives considered

- **Keep successor threads and fix each rule.** Rejected: ADRs 0493, 0503 and
  0507 were three rounds of that, and each fixed one stall while adding a rule.
- **A bespoke divider row for the boundary.** Rejected for the existing New
  session card: a second look for the same concept splits the transcript's
  vocabulary.
- **An engine API for per-session cost.** Rejected: the dev workspace widget
  reads the captures directly, and no other surface needs it yet.
- **Replace Claude Code's auto-compact with a handoff.** Out of scope. The
  estimate's formula and the auto-compact trial stay as they are.
