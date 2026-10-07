# 0357: A rebuilt child report reads each change's status now, not when the card was sent

- **Status**: Accepted
- **Date**: 2026-10-04

## Context

A trigger thread spawned a coding-agent child, which proposed a change. The
user applied it five minutes after the child's `ChildThreadCompleted` landed.
Three hours later, and again two days later, the parent told the user the
change still waited for Apply.

The `[CHILD THREAD COMPLETED]` block is rebuilt from the event every time the
parent resumes. Its `Pending changes:` line printed the ids the child left, with
no word on what happened next. The child's own summary said "pending your
Apply". The parent repeated it, then repeated its own earlier reply. ADR 0279
called the card a snapshot that "says so", and left the live answer one
`changes` call away. The model never made that call.

## Decision

Every model-facing build of the block reads each listed change's status from the
`changes` table. A change that left Review since the report gets a mark:
`now applied`, `now discarded`, `now reverted`, `now set aside`, or
`now deleted` when its row is gone. A note then says the change no longer waits
for Apply, and that any claim in the conversation calling it pending is out of
date.

`EventStore::build_messages_now` is the one production builder, and the fan-in
wake reads the same statuses for its single row. The pure builder without
statuses is test-only.

## Rationale

The event stays immutable. What changes is the projection, and the engine
computes that. The thread-state flags already work this way, filled at read
time (ADR 0279's own first decision). A snapshot that "says so" relies on the
model noticing and making a second call. In practice it trusts the most recent
claim in its context, which was its own stale reply.

The note is needed as well as the mark. The child's summary and the parent's
later turns sit in the same context and still say "pending". Without the note
the model weighs them against one parenthesis.

## Consequences

- ADR 0279's "the card is a snapshot" no longer holds for change status. The
  sub-thread line's `settled` / `still working` label is still the state when
  the card was sent. Once a change has moved on, its mark replaces that label.
- A status change rewrites an earlier message, so the prompt cache misses from
  that card onward, once per status change. ADR 0084 allows this, since the
  value comes from persisted state.
- Each history build costs one indexed query, skipped when the thread has no
  child report.
- A failed status read logs and renders the block as the child left it.

## Alternatives considered

- **Tell the model to check the `changes` list before claiming a change waits.**
  A prompt rule is advice the model already had through ADR 0279's wording, and
  ignored.
- **Wake the parent on `ChangeApplied`.** It costs a full parent turn per apply,
  for news the parent needs only when it next speaks.
- **A separate per-turn "change state" section.** It would correct the record
  from a second place, while the card kept saying the opposite. Fixing the card
  leaves one statement.
