# 0378: A blocked Archive or Delete shows its reason, never hides; the user resolves every blocker, and the menu cancels none

- **Status**: Accepted
- **Date**: 2026-10-06

## Context

A tester on the packaged build ran a thread family of a dozen sub-threads. He
could not archive or delete it, and asked whether he was missing something. He
was not. The thread menu hid Archive and Delete whenever a gate refused them,
and said nothing. The code called that "hidden rather than offered and
refused". Archive all (ADR 0349) kept threads back, and its "nothing to
archive" toast gave a count with no reason.

Every gate was right. A running turn, a pending question (ADR 0259), a change
awaiting Apply, and the home thread (ADR 0362) each block for a reason. What
failed was the silence: the user could not tell a gate from a bug, nor find
which of a dozen sub-threads held the family back.

`docs/plans/2026-10-06-blocked-thread-actions-explain-or-confirm.md` makes this
executable.

## Decision

**A blocked Archive or Delete is shown disabled, with its reason. It is never
hidden.** When sub-threads block, the menu lists every loaded one with its
title and state, strongest blocker first, and a tap opens it. A family can hold
several blockers at once, and naming only the first hid the rest until it was
resolved. A refusal toast still offers *Show sub-thread*, which opens the first.

**The user resolves every blocker themselves.** Nothing in the menu cancels a
question, stops a turn or discards a change.

**One classification names the reason**: the *action blocker*
(`thread_lifecycle::action_blocker`), generated to TS and cross-validated. The
engine's archive and delete refusals carry its slug as `blocker`, so the menu,
a refusal toast and Archive all say the same words. When several blockers
hold, it names the strongest, by a fixed priority.

**A thread in the Archive section offers Move to Current where Archive was**,
in the menu and the composer row. It brings the thread and its sub-threads
back, as Archive took them.

**Placement is where the drawer shows the thread, not where it is stored.** A
thread stored archived but kept in Current still offers Archive. It is blocked
by what keeps it there, or enabled when nothing visible does. Pressing it makes
the engine recount the family, so a drifted count lets the thread go. "Already
archived" is never the reason for a thread the user sees in Current.

## Rationale

**A gate the user cannot see reads as a bug.** The tester's own words were "not
sure if I'm missing something as a user". A disabled item with a sentence
answers that question where it was asked.

**Resolving a blocker is a decision the user should make on purpose.** A
pending question is a decision the agent cannot take alone. A running turn is
work in flight. A pending change is code. Each has its own control already:
answer, Stop, Apply or Discard. A cancel buried in Archive's confirm would
decide one of those as a side effect of tidying up.

**One definition keeps the reason honest.** The menu decides what to draw and
the engine decides what happens. If each worded its own reason, the two would
drift, and a stale client would claim a reason the engine never gave. So the
engine's refusal carries the slug, and the client words it from one table.

## Consequences

- The menu derives the blocker from the loaded family. When a sub-thread blocks
  but is not loaded, the item stays enabled. The engine's 409 then names the
  blocker, and the toast offers Show sub-thread. The server always decides.
- ADR 0259 stands unchanged: a thread waiting on the user is never archived. It
  is now visible why.
- Archive all still never cancels a question (ADR 0349). Its confirm and toasts
  count every kept thread with its reason, in the menu's words.
- The engine's decisions did not change. Only the refusal body gained a slug.
- Unarchive gained a second caller beside Archive all's Undo, and an opt-in
  scope that takes the sub-threads with it.
- The tester's second report was a thread stored archived yet kept in Current
  by a stale `active_children_count`, with no Archive and no reason. A switch
  abort keeps a child on its parent's count, and `ThreadArchived` set the child
  idle without a recount. It now recounts the parent, like any terminal. The
  boot rebuild heals rows that drifted before.

## Alternatives considered

- **Keep hiding the actions.** Rejected: it is the reported bug.
- **Allow Archive or Delete behind a confirm that cancels the pending
  question.** The first brief proposed it for the one blocker that looked
  cancellable. Rejected at plan review: the user should address the blocker
  explicitly. It was also less safe than it looked. Dismissing a question
  wakes a live agent, which carries on running, so the confirm would have had
  to stop the turn as well.
- **A new projection column to tell descendant blockers apart.** Rejected: a
  migration and two SQL mirrors to buy what the loaded family and the
  engine's refusal already give.
