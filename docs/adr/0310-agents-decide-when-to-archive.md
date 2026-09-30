# 0310: Agents archive a thread themselves, and decide when; no engine rule archives on apply

- **Status**: Accepted
- **Date**: 2026-09-27

## Context

A coding-agent thread whose change was applied stays in the thread drawer until
somebody presses Archive. `ChangeApplied` deliberately leaves the thread in the
inbox, so the button shows (`resolve_transition`). An orchestrating agent that
spawned and applied ten children leaves ten rows behind.

The first design was an engine rule: archive a coding-agent thread once its
change is applied, it has settled, and nothing is pending. It was dropped
before any code was written. Whether a thread is finished is a judgement about
the work, and the engine cannot see the work. It sees only the state.

## Decision

Agents get an archive action and decide when to use it:

- **The `threads` tool's `archive` action**, and `lucidos threads archive`
  through `POST /api/v1/threads/:thread_id/archive`.
- **The ladder is detach's** (ADR 0278). The caller is authenticated, never
  stated: the tool's own thread, or the verified origin token on the route. It
  may archive **itself** or **one of its own direct children**, and nothing
  else. No standing instruction widens it. A route caller with no token is the
  user, exactly as on the detach route.
- **It is the Archive button's cascade**, not a copy. The same gate refuses the
  same states with the same slugs, `parent_not_archivable` for a thread waiting
  on the user (ADR 0259). The target's own sub-threads go with it, as they do
  for the button. ADR 0312 narrows this for a pinned thread: an agent is
  refused a pinned target (`thread_pinned`) and leaves pinned sub-threads open.
- **A direct child archives at once.** **The calling thread archives when its
  turn ends.** Its call records a persisted `ThreadArchiveRequested`, and the
  *archive request* resolver runs the cascade once the thread settles. A newer
  message or archive closes the request.
- **The event says who.** Every agent archive and request carries the actor
  `Api { mode: agent, source_thread_id: <caller> }`.
- **The judgement lives in the guidance.** The chat system prompt, the
  coding-agent prompt and `orchestrating-sub-threads.md` say: archive a thread
  once its change is applied and no follow-up is expected. Leave it open while
  a follow-up, a question, a pending change or a live event wait remains.
  ADR 0330 removed this guidance: no prompt or knowhow says when to archive.

## Rationale

**The agent knows whether it is done, and the engine does not.** An applied
change often opens the next step: a review round, a second slice, a follow-up
the orchestrator is about to send. An engine rule would archive in that window.
The orchestrator would then address an archived child, and the worktree
cleanup would already be counting down to reclaim that child's tree.

**Archive is not free for a coding-agent thread.** Archiving opens the
worktree retention gate. A merged, clean worktree goes about an hour after the
thread idles, and a later follow-up rebuilds it cold. The decision belongs to
whoever knows whether a follow-up is coming.

**Self-archive has to wait for the turn to end.** A thread is always mid-turn
when its own tool or CLI call runs. The cascade gate refuses a running thread.
Even without the gate, the turn's own end would undo the archive:
`ResponseGenerated` and `CodingAgentIdled` both move a thread back to the
inbox. So the call records a request, and the archive lands after the settle.
The request is an event, not memory, so a restart keeps it (the statelessness
rule).

**One cascade.** An agent pressing Archive must meet the button's gate, not a
second one written to agree with it. The two would drift, and each side's tests
would still pass.

**Direct children, not descendants.** The ladder matches detach and follow-up
(ADR 0043). A grandchild is its own parent's to archive. The Archive route's
wider reach for a token-bearing caller (ADR 0168) stays as it is; this action
does not use it.

## Consequences

- Applied threads leave the drawer when an agent archives them. Nothing
  archives them on its own.
- The chat and coding-agent prompts each carry one more rule, paid on every
  turn.
- `ThreadArchiveRequested` is a new persisted event. The resolver is a bus
  subscriber beside the standing apply resolver, and reuses its settle probes.
- A self-archive request made while a change is pending waits until the change
  is applied or discarded, then lands.

## Alternatives considered

- **An engine rule on `ChangeApplied`.** Rejected, see Context and Rationale.
  It also needed an event-order rule to keep a reopened thread open, plus a
  bounded recovery sweep. All of that guesses at intent the agent has.
- **Children only, no self-archive.** Smaller: no new event, no resolver. It
  leaves a top-level agent thread with no way to close itself, which the
  request asked for.
- **Archive the calling thread at once.** The gate refuses a running thread,
  and the turn's end would move it straight back to the inbox.
- **Reuse `POST /api/v1/threads/archive` for the CLI.** Its token-bearing reach
  is self, every descendant and anything under a standing instruction. The
  action here is narrower on purpose, and changing that route's reach would
  change it for every existing caller.
