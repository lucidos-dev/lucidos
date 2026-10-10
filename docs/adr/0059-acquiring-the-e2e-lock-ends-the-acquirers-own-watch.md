# 0059: Acquiring the e2e lock stands down the acquirer's own watch for its release; the stand-down verb is by event type, not all

- **Status**: Accepted
- **Date**: 2026-08-10

## Context

ADR 0057 gave a refused e2e run somewhere to wait: the lock announces every hold
as a domain event, and the loser subscribes to `E2ELockReleased` and ends its
turn. That opened the loop and nothing closed it.

On 2026-08-09 a coding-agent thread subscribed at 18:31:16 with a 6h timeout.
At 18:38:27, seven minutes later, **it took the lock itself** and ran the browser
suite, and it took the lock eight more times over the following half hour. Its
change was applied at 19:14:45. At 21:21:38 the subscription nobody had stood
down matched an unrelated release and re-opened the thread, which spent seven
minutes re-running a spec and re-hardening a branch whose diff with `main` was
empty. Between 19:14 and 21:21 the thread read **Waiting** in the UI with all of
its work finished and landed, which is how the user found it.

The `e2e-lock-wait` skill already had a section on this, written after an
earlier six-hour instance of the same shape. Guidance had been tried and had
failed, in the exact case the guidance described.

## Decision

1. **Taking the lock stands down the acquiring thread's own `E2ELockReleased`
   subscriptions**, from `acquire_e2e_lock` on both the fresh and the reclaim
   path. Best effort, bounded, backgrounded, and addressed under the environment
   captured when the lock was taken, exactly like the two announcements beside
   it.
2. **Only that event type.** The stand-down may never reach `all`.
3. **On the reclaim path it runs FIRST**, before the release announced on the
   dead owner's behalf.
4. **The agent cancel surface gains a third target, `on`**: stand down every
   subscription on this thread watching one event type, whatever `condition`
   each carries. On the CLI as `lucidos event-waits cancel --on <EventType>` and
   on the `cancel_event_wait` LLM tool as `on`, over one shared engine function.
   Exactly one of `wait_id` / `on` / `all` is required, as before.

## Rationale

**Holding the lock IS the answer to a watch for its release**, and that is the
whole argument. The subscription asks one question, "is the lock free?", and an
acquire answers it definitively. Unlike every other way a watch can become
spent, this one is a fact available to the code at the moment it becomes true,
so it needs no model judgment and no memory of a rule read hours earlier. That
is the line between what belongs in a mechanism and what belongs in the skill:
the skill still owns the cases where the answer arrives some other way (the user
says skip it, the work is abandoned, a targeted spec is enough), because only
the thread knows those happened.

**Never `all`, because a thread can be waiting on more than one thing.** A run
may hold a watch on a release build or a sibling thread while it runs e2e, and
ending that silently is precisely the harm ADR 0052 exists to prevent: a watch
disappearing with nothing anywhere recording that the user did not ask for it.
The lock earned the right to end one watch and no others.

**The reclaim ordering is load-bearing, not tidiness.** That path emits an
`E2ELockReleased` on the dead owner's behalf, which is exactly the event our own
watch matches. A watch still live when it lands wakes the thread onto a lock the
thread is holding. Standing down first is what makes a reclaim unable to wake
its own reclaimer. On the fresh-acquire path the order changes nothing (nothing
it emits can match), and it is the same order there anyway, because the rule is
"the watch is answered when the lock file is written", not "when something is
announced".

**`on` had to exist, and its absence had already cost something.** The surface
offered "one id" and "everything". The id has to be read out of `list` first,
which a script cannot sensibly do (it would mean a JSON parser in the lock's
acquire path, keyed on a `reason` string the agent wrote freehand), and
"everything" is the one thing the lock must not do. So "I no longer need to be
told about X" was not expressible at all. Adding it is not a concession to the
script: it is the safe verb for the request the chat agent actually gets, since
"stop watching for the release build" otherwise costs three steps and a uuid
carried between them, and an agent that skips the list to save a step lands on
`all`.

**Both surfaces get the argument because they share the refusals.** The engine
function is one, so a `wait_id`/`on`/`all` refusal naming `on` would otherwise be
addressed to an argument only the CLI had. That is the drift ADR 0052 §3 named
when it insisted the three verbs be mirrored.

**The stand-down is best effort for the same reason the announcements are.** An
e2e run must not go red, and an EXIT trap must not stall, because the engine was
briefly unreachable. A stand-down that fails leaves exactly the state that
existed before this ADR, which the subscriber's own timeout still recovers.

**A refusal is the ordinary path and is discarded.** Most runs never lost the
lock, so most acquires stand down nothing. `on` refuses an empty match, on the
same footing as `all` with nothing live, because for an agent that believed it
was watching, "nothing is watching for X" is the fact the surface exists to
report. The lock discards both the output and the status.

## Consequences

**Kept.** Every gap ADR 0057 accepted. A holder in another workspace still
cannot wake a waiter, the emit is still lossy, and the subscriber's
`--timeout-secs` is still the recovery for both. Ending a subscription is still
announced (ADR 0052): the stand-down travels the ordinary `cancel_event_wait`
path, so each stop writes `EventWaitCanceled` with cause `AgentStandDown` and
leaves its transcript line. The three legs that scope these verbs to the calling
thread are untouched; `on` adds no way to name another thread.

**Paid.** 245 characters on the always-loaded chat prompt for the tool's third
argument, and the ratchet in `system_prompt.rs` moved to 106,150 with that
reasoning recorded. Up to one more bounded CLI call in the teardown's wait, on
top of the one or two already there.

**Given up.** Nothing about the case where a thread never takes the lock. That
watch is still the thread's to end, and the skill still says so.

**Cause reused rather than added.** `AgentStandDown` covers it: the lock library
runs inside the agent's own subprocess and calls `lucidos event-waits cancel`,
which is what that arm already documents.

## Alternatives considered

**Cancel a thread's subscriptions when its change is applied.** It would have
fixed this incident too, and it was rejected as both too broad and too late. Too
broad, because ADR 0052 fixed the set of things that legitimately end a
subscription (archive and discard, plus the two explicit stops) and Apply is not
a thread ending: the user can carry on in the thread afterwards. Too late,
because the watch here was already dead weight at 18:38, thirty-six minutes
before the Apply, so an Apply-time sweep would have treated a symptom that had
been visible for over half an hour.

**Have the engine's event-wait dispatcher cancel on `E2ELockAcquired`.** One
place, no CLI round trip, and testable in Rust. Rejected as a layering
violation: the engine knows nothing about the e2e lock today, which is only a
protocol between shell scripts expressed in generic domain events, and a
hardcoded event name in the generic dispatcher is the first of a class.

**Parse `event-waits list` in bash and cancel the matching ids.** No engine
change and no restart, which was the whole appeal. Rejected because it puts a
hand-rolled JSON parser in the acquire path for a document containing a freehand
`reason` string that can itself contain the word `event_type`, and because it
treats a missing verb as the script's problem to work around rather than as a
gap in the surface.

**Expose `on` on the CLI only, keeping it off the LLM tool to save prompt
budget.** Rejected: the two callers share one function and therefore one set of
refusals, so the chat agent would read about an argument it did not have. The
budget is real and the ratchet is deliberately tight, so the raise is recorded
with its reasoning rather than waved through.

**Make an empty `on` match a quiet success instead of a refusal.** Tempting
because the lock's ordinary path is a refusal. Rejected for consistency with the
empty `all`, and because the refusal carries information for the caller that is
NOT a script: an agent that believed it was watching has just learned otherwise,
which is the thing this whole surface was built to tell it.

**Let `--on` be repeatable, matching `await-event --on`.** Rejected as
speculative: one event type per call, and a caller that wants two calls it
twice.

**Re-arm the remainder when `on` names one type of a multi-type
subscription.** A subscription can watch several event types, and naming one of
them ends the whole thing, which reads at first like `on` breaking its own
promise to leave other watches alone. Rejected, and the reason is what a wait
IS: one rendezvous with several triggers, spent by the first match, not several
independent watches sharing a row. Once you have stopped watching for its `A`
leg there is no `B` leg left that could still wake you, so the only way to
"keep the rest" is to replace it with a narrower subscription the caller never
armed, with a new id, a new watermark and a `reason` somebody else wrote.
Nothing in this family mutates a wait, deliberately: the persisted
`EventWaitStarted` IS the wait (ADR 0047), and there is no update verb anywhere
in the surface. What the case needs instead is honesty, which it has: the
result names every event type it ended, so a caller that wanted the rest can
re-arm it. Recorded in `docs/code-review-priors.md`, because it looks like a
bug from the diff alone and a review found it there on 2026-08-10.
