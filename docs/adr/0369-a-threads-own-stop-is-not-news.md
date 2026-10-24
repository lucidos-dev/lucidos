# 0369: A thread's own stop of a background task does not wake it, and the hardened marker stops the thread's leftover tasks

- **Status**: Accepted
- **Date**: 2026-10-05
- **Amends**: [0257](0257-coding-agents-wait-through-engine-owned-background-tasks.md).
  A stop no longer delivers to the thread that issued it, and `lucidos hardened mark`
  ends a thread's leftover background tasks.

## Context

ADR 0257 arms an engine *event wait* on every coding-agent background task when it
starts. That wait has two failure shapes, and one thread hit both inside three minutes.

1. A Claude Code session started `make lint` as a background task, then ran
   `/harden`, which runs `make lint` itself. It recorded the marker and ended its turn
   with "ready to Apply". The redundant task and its wait were still live. The thread
   read **Waiting**, and ADR 0106 withheld **Apply** for as long as the wait lived.
2. The user pointed out the wait. The agent ran `lucidos background-task stop`. The
   SIGTERM reap emitted `BackgroundBashCompleted { killed: true }`, the wait delivered
   it, and a whole new turn opened to say "that was the task I stopped".

Nothing was wrong with any single step. The engine had no notion of a stop being the
thread's own, and nothing reconciled a thread's background work with the thread
declaring itself finished.

## Decision

1. **A thread's own stop stands down its own watch.** The stop comes from `lucidos
   background-task stop` or the chat agent's `bash_kill`. It first stands down every
   live wait on the *issuing* thread that would fire on the stopped task, and only then
   sends the signal. The cause is `AgentStandDown`. The completion is still emitted and
   recorded. It matches nothing on the thread that asked for it.
2. **The engine narrows its own wait, never a model's.** An engine-armed wait that also
   covered other tasks is replaced by an engine wait over the rest. The replacement keeps
   the original watermark and deadline, and is written before the old wait's cancel. A
   wait the model armed that watched more than the stopped task ends whole, and the stop's
   result names it.
3. **A task with a stop in flight is never armed for again.**
4. **`lucidos hardened mark` stops the calling thread's leftover background tasks**
   through the same path. Its reply names what it stopped and every wait still live on
   the thread. A caller with no thread stops nothing.

## Rationale

**Stopping a task is the answer to a watch for its completion.** It is the same
argument as ADR 0059, where taking the e2e lock answers a watch for its release. The
fact is available to the code at the moment it becomes true, so it needs no model
judgment. The cause is reused for the reason 0059 gave: the stop runs inside the
agent's own turn, and `AgentStandDown` already reads as "the agent stopped it".

**The order is what makes it structural.** A child can exit on SIGTERM and have its
completion emitted within milliseconds. A stand-down after the signal would race the
delivery it exists to prevent. So the registry splits a stop in two:
`begin_stop` takes the stop signal and marks the task *stop requested*, the waits stand
down, and then the signal goes out.

**Only the issuer's waits.** A stop from another thread is news to the owner. So are
the watchdog's timeout, Discard, Archive, `stop_agent` and the teardown, none of which
is the thread's own stop. All of those still deliver.

**Narrowing the engine's own wait does not reopen ADR 0059's objection.** 0059 refused
to replace a wait with "a narrower subscription the caller never armed". For an
engine-armed wait the engine IS the caller. It wrote the `on` list and the reason, and
it re-arms that wait every turn anyway. Keeping the original watermark is what makes it
lossless: the replacement's catch-up scan reaches any completion of the remaining tasks
since the original was armed.

**The replacement is written before the cancel.** The thread then never reads as
not-waiting in between, so a waiting child never sends its parent a card early
(ADR 0254). A model's wait keeps 0059's rule unchanged.

**A stop in flight must not be re-armed.** `stop X; run Y` is a natural command line.
Without the *stop requested* filter, the arming for Y found X still in its grace. It
armed a wait on X, and X's killed completion woke the thread anyway.

**The hardened marker is the thread's own declaration that it is finished.** `/harden`
runs once, last, over the finished batch. A background task still running at the marker
started before the certification, so its result can no longer change the verdict.
Leaving it running is exactly the state that withheld Apply. The engine learns this at
the marker, before the agent writes its summary, so the agent can report what was
stopped.

**A wait the engine cannot judge is named, not ended.** A deliberate long build, a
child thread or an e2e lock may be the reason the thread waits. Ending it unasked is
the silent stop ADR 0052 exists to prevent. Blocking the marker on it would make the
engine judge what only the thread knows. So the reply names it and states what it
costs: the thread stays waiting, and Apply stays withheld.

## Consequences

- A turn that ends after `/harden` cannot hold a stale background-task wait, so Apply is
  not withheld by work the session already certified past.
- An agent that stops its own task gets no extra turn for it. Its stop result says so.
- `CodingAgentIdled.bg_bash_pending` no longer counts a task the thread already stopped.
- Worktree cleanup and the todo consumer still see a stopping task as running, which it
  is, until it is reaped.
- A model's multi-entry wait over a stopped task ends whole. The stop names it, and the
  agent can re-arm the rest.
- Each narrowing writes one more `EventWaitStarted`, which counts toward the
  recent-subscription cap. It needs one stop per task, so it cannot loop.
- If writing a replacement fails, the old wait goes back into the cache and the stopped
  task's completion reaches it: the behaviour before this ADR. If the cancel fails after
  the replacement is written, the replacement is withdrawn and the old wait restored. Only
  a second failed write leaves both live, so that a remaining task could deliver twice. Every
  one of these is logged.
- `mark-hardened` answers 200 with a JSON report instead of 204.

## Alternatives considered

- **Tag the completion with who stopped it, and have the dispatcher skip the issuer's
  waits.** It keeps every wait untouched. But a wait whose only entry was the stopped
  task then sits until its deadline and expires into a turn, which is worse. It also
  puts a special case in the generic matcher, and the boot catch-up scan would have to
  reproduce it.
- **Stand down after sending the signal.** Simpler, and it loses the race on any task
  that exits promptly on SIGTERM.
- **Refuse the hardened marker while background tasks or waits are live.** It leaves the
  fix to the model, which is the failure this replaces. A refused marker also makes Apply
  run hardening synchronously, at the user's expense.
- **Stop the tasks at idle instead of at the marker.** By then the agent has written its
  summary and cannot report what was stopped. Idle also cannot tell a certified turn from
  one deliberately waiting on a build.
- **End every live wait at the marker.** A child-thread or e2e-lock wait may be the
  point of the wait, and ending it unasked is the silent stop ADR 0052 forbids.
- **A new UI state for "finished but still waiting".** The Waiting dot, the waiting row
  and the withheld Apply already say it. The defect was waits that should not exist,
  not a missing way to show them.
