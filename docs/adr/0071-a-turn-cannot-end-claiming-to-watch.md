# 0071: A turn cannot end leaving work open with nothing to wake it

- **Status**: Accepted
- **Date**: 2026-08-13

## Context

The Lucidos Agent ends a chat turn saying it will keep watching, while holding
no subscription. Nothing re-opens the thread, so the promised follow-up never
comes and the thread reads as finished. Seen twice, both on a turn re-entered by
an `EventWaitDelivered`.

Prose was tried first and three separate sentences already say it. The wake
notice (`WAIT_SPENT_NOTICE`) says "Narrating it does not do it: a turn that ends
with no new call leaves nothing watching for this, whatever the sentence said".
The `await_event` description says "Saying you will re-subscribe is not
re-subscribing". The chat system prompt says "Do NOT end the turn with 'I'll
report when it finishes' and no call". All three were in context both times.

Meanwhile the engine already computes the fact, one moment too late.
`settle_open_todos` runs at every chat terminator and asks whether the thread
holds a live *event wait* or unfinished background work. If neither, it rewrites
every open todo item to `Abandoned`. On the second incident it stamped
`abandoned` onto three items reading "Wait for X", at the instant the reply said
"I am watching". That verdict reached the todo panel and nothing else.

## Decision

The engine evaluates that same predicate one round EARLIER, in the agentic
loop's no-tool-calls termination branch, and calls it the **wake check**. It
sends a turn back once when three things hold: open todo work, no live event
wait, and no unfinished background task. The agent then arms a subscription,
settles the list, or says plainly that it is not watching.

Once per turn. Past the bound the turn finalizes and the settle records
`Abandoned`, exactly as before.

## Rationale

**The signal is already deterministic, so no new judgement is invented.** The
predicate is the `Abandoned` branch of a check that has run on every chat
terminator for months. Moving its evaluation earlier costs one probe and gives
the agent the one thing it lacked: the fact, while it can still act on it.

**Before the terminator, not after.** The user sees ONE reply, the corrected
one. A post-terminator re-open would add a second assistant message, wake the
thread, and possibly notify, all to correct a paragraph the user is still
reading.

**The precedent is in the same branch.** `should_force_question_reask` already
rejects a final answer, pushes the drafted prose plus a forcing user message,
and continues the loop. The wake check is its sibling and shares the shape,
including the per-turn bound that stops a non-complying model trapping the loop.

**A Stop during the final LLM call skips the check.** The loop's only cancel
check is at the top of a round. Nudging would spend the drafted answer just to
reach it, and end the turn as cancelled. Today that answer lands.

**A queued follow-up outranks it, so it sits after the injection drain.** A
message the user sent mid-turn re-opens the thread by itself. Checking first
would deny that while the message sat queued, which is false. It would also
cost the follow-up a round of waiting.

## Consequences

- A chat turn that leaves work open with nothing to wake it costs one extra LLM
  round. In this workspace that is a handful of turns a day.
- The todo panel gets more honest for free. A turn nudged into settling its list
  no longer leaves a misleading `Abandoned` trail.
- `latest_todo_list` is now shared by the wake check and the settle, so the two
  cannot come to disagree about which list is current.
- The two readers of "will anything re-open this thread" stay deliberately
  different. The wake check reads the in-memory registry, because it runs inside
  the turn where there is no terminator to be as-of. The settle keeps its
  sequence-scoped anti-join, because it is an async consumer.
- The existing prose stays. It is cheap, and a model that never needs the gate
  is a better outcome than one the gate catches.
- Coding-agent threads are structurally unaffected: they never emit
  `TodoListWritten`, so the probe reads nothing.

## Alternatives considered

**Detect the promise in the reply text.** Rejected on the corpus. Over 30 days
the settle wrote 365 abandoned items and only 25 begin with "wait" or "watch".
The downstream steps of the same stall ("Phase B: promote and publish") carry no
marker at all. A text test would miss most of the damage and fire on innocent
wording, which is the worst of both.

**Let the engine arm the wait itself**, as `event_wait/background_task.rs` does
for a running background task. Not possible here. That case works because the
engine holds the task id and can build the subscription. This one needs an event
type and a condition that only the agent knows.

**Re-open the thread after the terminator instead.** Cheaper to build, worse to
live with: a second assistant bubble correcting the first, a thread that wakes
itself, and a notification for a turn the user just read.

**Leave it to prose and keep sampling.** This is what the temporary-measures
entry proposed, and its removal condition has now been tested twice by reality
and failed both times. A fourth sentence would be the same bet again.

**Nudge on every abandoned settle, including after the turn.** Rejected as the
same thing one moment too late, and noisier: it would fire on threads whose turn
already ended cleanly hours ago.
