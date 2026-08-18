---
name: e2e-lock-wait
description: Use when an e2e script refuses to start because another run holds the e2e lock ("ERROR: another e2e run is in progress", "orphaned processes"). Subscribe to E2ELockReleased with `lucidos await-event` and END THE TURN instead of sleeping, polling, re-running the script on a timer, or writing a retry loop into /tmp. Covers picking a timeout, what to do on the delivery, and the two cases where nothing will reach you.
---

# You lost the e2e lock. Subscribe, do not poll.

One e2e run at a time, machine-wide. Two concurrent Playwright sessions OOM'd a
32 GB Mac on 2026-04-19, so `scripts/lib/e2e_lock.sh` makes the second entrant
exit 1. That refusal is correct and is never worked around: **never delete the
lock file, never force it, never re-run with a flag that skips it.**

What is wrong is *waiting by polling*. On 2026-08-09 two coding-agent threads
lost this race at once. One wrote `/tmp/run-e2e-retry-<pid>.sh` with
`for i in $(seq 1 120)` around `./scripts/e2e-browser.sh` and `sleep 20` on
refusal, holding a foreground tool call open for 40 minutes and re-executing the
entry script's build checks on every attempt. The other parked on a bare
`sleep 20`. Both burned a turn and held engine capacity to learn something the
engine could have told them.

## Do this instead

**1. Confirm the lock is actually held.** The subscription watches FORWARD only.
Subscribing to a release that already happened means waiting for a second one
that may never come.

```bash
LOCK="${E2E_WORKSPACE:-$HOME/workspaces/e2e-test}/.lucidos/e2e.lock"
cat "$LOCK"                      # no such file => it is free, just run the script
kill -0 <PID from the file>      # non-zero => the owner is dead, the next run reclaims it
```

**2. Subscribe.**

```bash
lucidos await-event --on E2ELockReleased --timeout-secs 21600 \
  --reason "the e2e lock to free up (held by e2e-browser since 14:12)"
```

**Name what you await, not the fact that you await it.** The transcript labels
the reason `Set up an event wait: <reason>` and `Stopped waiting: <reason>`, so
a reason opening "waiting for" reads as `wait: waiting for`.

**3. END THE TURN.** Say what you subscribed to and stop. This is the whole
point, not a side effect: the command returns immediately and blocks nothing, so
a turn that keeps working after it is a turn nobody needed to hold open. The
thread sits plain idle while it watches.

**4. On the delivery, retry the script exactly once.** It arrives as a new turn
with the whole conversation behind it, so re-read what you were doing first. If
the retry is refused again, another waiter won the race: go back to step 1.

**5. Nothing to clean up once you win, and nothing to say about it.** Taking the
lock stands your own watch for its release down, whether the delivery or
something else got you there. The transcript records that stand-down on its own
card, so do not narrate it: a line explaining that the lock took care of your
watch is the third telling of one fact. What still needs you is the case where
you never take it at all: see "A watch you no longer need is yours to end".

## The sharp edges

Get these wrong and you are worse off than with the sleep loop.

### It is one-shot, and re-subscribing is normal

The first match consumes the subscription. One release is delivered to every
waiter and they race; exactly one wins the lock. Being refused on the retry is the expected
case, not a failure, and it is not a reason to fall back to polling.

Nothing is watching after a delivery. If you say you are still waiting, call
`await_event` again in that same turn: narrating it does not do it.

### A watch you no longer need is yours to end

The one-shot rule cuts the other way too, and this is the half that gets
forgotten. A subscription is spent by a MATCH, and by nothing else: not by the
turn ending, not by the user sending you something else, and not by you going on
to run the suite some other way. It outlives all three on purpose (ADR 0052).

**Winning the lock is handled for you.** The one case this used to cost the most
is now mechanical: `acquire_e2e_lock` stands your watch for `E2ELockReleased`
down the moment it takes the lock, because you cannot hold the lock and still
need to be told it is free. You do not have to remember it, and you do not have
to check afterwards. Only that watch is ended, never anything else you are
waiting on.

So what is left for you is every OTHER way the answer can arrive: the user tells
you to skip the suite, the work is abandoned, the run happened somewhere else,
you decide a targeted spec is enough and never take the lock at all. Stand it
down yourself there:

```bash
lucidos event-waits list                             # the ids, reasons and deadlines
lucidos event-waits cancel --on E2ELockReleased      # or --wait-id <id>, or --all
```

Prefer `--on`: it needs no id, and unlike `--all` it cannot end a watch you
armed for something unrelated.

What this cost before the acquire did it for you: you subscribe, the user comes
back and resumes you, you retry and win the lock, and the suite runs green.
Nothing resolved the subscription, so the thread reads **Waiting** for the rest
of your timeout after its work is finished and applied. Six hours in the
incident that produced this section, and two hours plus a spurious re-open in the
one that produced the automatic stand-down. Cancel it in the turn you stop
caring, and say you did.

### It watches forward only, and the command tells you what you missed

Registration scans the **3 minutes before** the call and reports any match it
finds, under a heading that says `ALREADY HAPPENED`. That report is the part of
the output you have to act on, and it is printed above the "subscribed"
confirmation. Read it. If the lock was released while you were composing the
call, the subscription will never fire for it: go retry the script now instead of
waiting.

### There is a cap of 10 consecutive subscriptions

Ten registrations with no message from the user, and the eleventh is refused
outright. So do not spend the budget on short waits and do not treat
re-subscription as free:

- **Pick the timeout from the run you are waiting on, generously.** A full
  browser suite is hours (the 2026-08-09 nightly leg ran 5h17m). `21600` (6h) is
  a sane default for a full suite, `7200` (2h) for a targeted run. The hard cap
  is 86400 (24h).
- **Cap your own attempts at 3 for one e2e run.** After that, stop and tell the
  user the lock has been held for N hours by which script and worktree. A wedged
  lock is theirs to decide about, and it is invisible to them while you keep
  quietly re-arming.

### The timeout is a backstop you must handle

On expiry the engine re-opens the thread with a timeout notice. Do not silently subscribe
again. Read the lock file, then report: released and you missed it, still held by
the same owner, or held by someone new. Re-subscribe only if the answer gives you
a reason to expect a release soon, and only within the attempt cap.

## Two cases where nothing will reach you

Both are real, both are known, and in both the `--timeout-secs` deadline is the
recovery. That is why the timeout is required and why it must be a number you are
willing to be re-opened on.

**The holder is in another workspace.** The lock is shared by every workspace on
the machine, but `lucidos events emit` writes to the emitting subprocess's own
`$LUCIDOS_WORKSPACE`. A holder in workspace A releasing does not reach a waiter in
workspace B, and there is deliberately no cross-workspace emit: do not invent one
and do not POST to another engine's port to fake it. Compare the lock file's
`WORKTREE` with your own `$LUCIDOS_WORKSPACE`; when it is elsewhere, the refusal
message says so too. Use a shorter timeout there (say `1800`) and plan on
reporting at expiry rather than on a delivery.

**The engine was down at the moment of release.** The emit is best effort by
design, so nothing is written and there is nothing for your subscription to catch
up on when the engine returns. Narrow in practice, because a delivery only works
same-workspace, which means your own workspace was down with it.

The neighbouring case is covered and needs no special handling: a holder killed
hard enough to skip its EXIT trap leaves a stale lock, and the next run to
reclaim it emits the release on the dead owner's behalf with
`outcome: "reclaimed"`.

## What survives a restart

The **subscription** does. It is persisted as an event, rebuilt from the event
store when the engine boots, and it carries the sequence it was armed at, so a
release emitted while the engine was down is still delivered on the way back up.
A deadline that passed during downtime re-opens the thread with its timeout
rather than vanishing. So a restart is not a reason to re-subscribe or to go back to polling.

The **emit** does not, per the case above.

## The events

Both are domain events, emitted best effort by `scripts/lib/e2e_lock.sh`.

| Event | When | Payload |
|---|---|---|
| `E2ELockAcquired` | a run takes the lock | `script`, `thread_id`, `worktree`, `reclaimed` |
| `E2ELockReleased` | a hold ends | `script`, `thread_id`, `worktree`, `held_secs` (absent on an old lock file), `outcome`: `released` or `reclaimed` |

Subscribe to `E2ELockReleased` with no `--condition`: any release frees the lock,
whichever script held it. Filter only if you genuinely care which one, and
remember a condition that never matches is indistinguishable from a lock that is
never released.
