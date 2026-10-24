# 0451: The spawn declares a child's wake mode: no-wake, wake or wake-when-all-done; the wake decision is the fallback for undeclared children

- **Status**: Accepted
- **Date**: 2026-10-12
- **Amends**: [0434](0434-sub-threads-wake-parents-only-when-needed.md)
- **Amended by**: [0473: A thread spawns a top-thread when no report is needed](0473-a-thread-spawns-a-top-thread-when-no-report-is-needed.md).
  Work this thread will not act on is a top-thread, not a `no-wake` child.
- **Amended by**: [0545: A no-wake child files every report](0545-no-wake-child-files-every-report.md).
  A `no-wake` child's failure, cancel or interruption is filed too.

Plan: `docs/plans/2026-10-12-the-spawn-declares-the-wake-mode.md`.

## Context

ADR 0434 had every child turn end with a *wake decision*: does the parent have
to act on this report now? The child said it, or one background call guessed.
ADR 0439 measured the guess on 31 synthetic reports. The decision models missed
no must-wake but woke on noise: OpenAI Decisions on 7 of 15 file cases, Jev on
11. Every chat model in the fallback chain missed real wakes.

The parent knew the answer when it spawned the work. It knows whether it waits
for the result, combines several, or hands the work to the user.

## Decision

The spawn declares the child's *wake mode*, and the engine applies it with no
model call.

- **Three modes**, on `run_thread`, `run_coding_agent` (`wake_mode`) and
  `lucidos spawn-thread --relation child` (`--wake-mode`):
  - `no-wake`: the user reads the result in the child. A clean report is
    filed.
  - `wake`: the parent needs the result to continue. Every report wakes it.
  - `wake-when-all-done`: the parent combines several results. A clean report
    wakes it only when no other child of the batch is still working.
- **Defaults**: `no-wake` under the home thread, `wake` under any other parent.
  `WakeMode::for_spawn` is the one definition, and every surface resolves
  through it. An HTTP spawn that omits the field gets the default too.
- **Stored once, at the spawn.** The mode rides the child's spawning
  `MessageReceived` beside `parent_thread_id`, and its projection writes
  `thread_summaries.wake_mode` in the same INSERT. It lasts the child's life.
- **Failures still wake, under every mode.** So do a turn the engine cut short
  and an empty reply: the engine records `ParentWakeRequested`, and delivery
  honours a recorded yes under every mode.
- **The wake decision is the fallback**, only for a child with no mode: one
  spawned before modes existed, or a spawn queued before them.
- **A child's own `wake_parent` call under a declared mode records nothing.**
  The tool and `WAKE_DECISION_RULE` reach only an undeclared child. A call that
  arrives anyway is answered with status `declared`.

## Rationale

**The spawn knows, so nothing has to guess.** A model call at every turn end
paid to recover a fact the parent held at spawn time. It got that fact wrong
both ways.

**A batch, from counts the engine already keeps.** A batch is every child of
one parent declared `wake-when-all-done` that is still working. "Still working"
is the predicate the wake decision already used for its sibling count: an
active status, or a live event wait. The fan-in runs after its child's terminal
commits. So whichever batch terminal commits last sees no sibling still
working, and wakes. Two terminals committing together can both wake, and can
never both file.

**A queued sibling is not counted.** It has no row. Leaving it out can only add
a wake, since it reports when it runs. Counting it could lose one: a queued
spawn the user cancels never reports.

**A card-less abort in a batch sends a failure card.** A safety-net or killed
process abort sends the parent nothing, by design, because the user sees the
child's error. In a batch that silence would strand its siblings' filed
reports. So the last working batch child that ends that way sends a `failure`
card.

**A child cannot escalate a declared mode.** Letting a `no-wake` child wake its
parent brings back the noisy guess, and the parent already said it does not
need the result. Failures wake on their own, and a child that needs the user
asks on its own card.

**Every turn follows the mode, whoever opened it.** Splitting by opener would
file the report of a turn a card answer opened. That turn may finish the work
the parent waits for.

**The name.** All three values say what happens to the parent's wake, so the
concept is the *wake mode*: `wake_mode`, `--wake-mode`, `WakeMode`, the column
and the glossary entry share one root.

**"Filed report" keeps its name.** It names an outcome, not the declaration,
and three paths produce one: a `no-wake` child, an early batch sibling, and the
wake decision's no. Renaming `ChildReportFiled` would cost a legacy alias, a
wire type and a UI line, for no new meaning.

## Consequences

- A declared child costs no wake-decision call, and no prompt text about one.
- A parent that needs an answer from a `no-wake` child spawns a new child with
  `wake`. A follow-up does not change the mode. The prompts say so.
- A batch can wake its parent twice: once when two siblings finish together,
  and once when a queued sibling runs after the others finished.
- A user Stop on the last working batch child wakes nothing, as a Stop never
  does: the child is paused, not finished. The batch wakes its parent when the
  user continues the child, or archives it.
- The wake decision stays, as permanent back-compat for old children. The
  tuning work on its chat fallback matters only for them.
- ADR 0434's "the child decides" now holds only for an undeclared child.

## Alternatives considered

- **The child decides at turn end (ADR 0434).** Kept only as the fallback. ADR
  0439 shows the guess is noisy in both directions.
- **A child may escalate a declared `no-wake` with `wake_parent`.** Rejected:
  it is the same guess, and models lean towards waking.
- **Defaults only on the tools, the wake decision for an HTTP spawn that omits
  the mode.** Rejected by the user: every new spawn gets a mode.
- **Count queued siblings in a batch.** Rejected: a canceled queued spawn never
  reports, so the batch would lose its wake.
- **Rename `ChildReportFiled` to follow `no-wake`.** Rejected: it is a
  different concept with three sources, and a rename costs an alias.
- **Backfill a mode onto existing children.** Rejected by the user's fallback
  rule: existing children keep the wake decision.
- **A follow-up that changes the mode.** Deferred. The spawn declares it, and a
  parent that needs a different mode spawns again.
