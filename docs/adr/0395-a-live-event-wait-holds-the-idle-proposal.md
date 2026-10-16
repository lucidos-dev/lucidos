# 0395: A coding agent's clean idle does not propose its change while the thread holds a live event wait

- **Status**: Accepted; an agent's archive request no longer waits on unproposed
  work a turn end withheld, per [ADR 0400](0400-coding-agent-change-state.md).
- **Date**: 2026-10-09

## Context

A coding agent that runs long work in the background starts it with
`lucidos background-task run` or `lucidos await-event`. Either arms an event
wait. The agent then ends its turn, and the wait re-opens the thread when the
work reports. One run can take many such turns: the nightly e2e step took five
over three hours.

The idle arm proposed the branch as a change at every clean turn end. So a
thread that said "Not done yet" kept producing change cards and summary runs.
Each change row went stale at the agent's next commit. Apply was withheld the
whole time anyway, because `available_thread_actions` refuses it while a wait
is live. The user read the cards as the engine "auto-committing turns".

An earlier gate refused to propose while a background task was pending. It was
removed in May (commit "remove bg-bash propose-gate so coding-agent changes
propose at idle"). It read an in-memory tracker that often missed a task's end,
which left threads with a real diff and no Apply button at all.

## Decision

A clean (`Generated`) idle on a thread holding a live event wait proposes no
new change. It still re-syncs a change already open on the branch, so that row
never goes stale under a standing apply. The held work is proposed later, by
whichever way the wait ends:

| The wait ends by | What proposes |
|---|---|
| Delivery or expiry | Both re-open the thread, and that turn's idle proposes |
| Stop waiting, or an agent stand-down | `propose_work_a_canceled_wait_held`, from the branch-work subscriber, credited to whoever canceled |
| Discard | The existing discard path |

A user Stop still proposes as incomplete (ADR 0328), wait or no wait.

**Held work outranks Archive and Delete**, as a proposed change did before the
hold. `own_blocker` takes a `ChangeWork` input and refuses with
`held_proposal`, whose copy says to stop waiting first. A parked thread with no
diff keeps Archive (ADR 0049).

**The parent hears the change.** A waiting child's held card is built inside
the cancel's own emit, before any proposal. So the cancel defers the card for a
child with held work. The branch-work net reads that, proposes, then sends the
card with the change listed.

## Rationale

**The wait is a durable signal, unlike the May tracker.** `EventWaitStarted` is
persisted before the CLI call returns, so the wait is in the live set before
the turn's `Result`. It survives a restart (ADR 0047). Every way it ends is an
event. The May gate failed because its "done" signal could be missed, and that
cannot happen here.

**Nothing is stranded.** Each way a wait ends leads to a proposal or to a path
that owns the work. The startup sweep skips a waiting thread rather than
undoing the hold, and still catches a cancel that a lagged subscriber missed.

**The proposals bought nothing.** Apply was already withheld while a wait was
live. So the held cards only cost a summary model call each, and left a change
row that was wrong by the next commit.

## Consequences

- A waiting thread shows no change card until its work is done. The thread
  card already shows "waiting" above "changes".
- A held child's card waits on the branch-work subscriber. A broadcast lag
  there leaves the card owed until the child's next turn, archive or discard.
- An agent's archive request waits on any unproposed diff, as the standing
  apply and Archive all do. That closes the moment between a canceled wait and
  its proposal. It also holds a request on a failed turn's diff until the user
  resolves it.
- A parent's `blocking_descendant_count` does not count a held child, so the
  menu offers the parent's Archive. The family gate refuses it and names the
  child, so the failure is loud.
- The turn-end auto-commit of dirty files is unchanged.
- A wait that resolves before its turn ends queues the re-entry as a follow-up.
  That idle still proposes. This case is narrow, and the next idle re-syncs it.

## Alternatives considered

- **Keep proposing at every idle.** This rejected the user's report as working
  as designed. The proposals cost a model call each and gave the user no action.
- **Gate on running background tasks again.** A task with no wait re-opens
  nothing, so holding on it could strand work. This is the May failure in a
  new form. The wait, not the task, is what promises another turn.
- **Hold the turn-end auto-commit too.** This was offered to the user and not
  chosen. The commit is invisible until a proposal surfaces it, so the hold
  alone removes what the user saw.
- **Let the archive net set aside held work.** Its guard skips any branch a
  change row ever named, to never resurrect a decided change (ADR 0328). After
  an apply on the same branch, it would therefore hide new held work. Refusing
  Archive keeps the thread in view until Stop waiting proposes the work.
- **Propose before the cancel at each cancel site.** Three sites emit a cancel,
  and one is a free function with no engine. Deferring the card to the
  subscriber that proposes covers all of them in one place.
