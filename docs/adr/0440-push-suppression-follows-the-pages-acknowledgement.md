# 0440: Push suppression follows the page's report that it handled the notification, not its presence

- **Status**: Accepted
- **Date**: 2026-10-12

## Context

The OS push decision and the in-app toast were two steps, a check and then an
act:

1. The engine broadcast `PresenceCheck` and waited up to `DEADLINE_MS` (2000)
   for pongs. A pong with `is_active: true` suppressed the push.
2. Only then did it broadcast `NotificationToastRequested`. The page checked
   again, and a hidden page dropped the toast.

A page that answered "active" and hid in between left the reader with
**neither** surface. Its answer suppressed the push, and its later check dropped
the toast. The window was the deadline plus SSE latency, and backgrounding a
phone right after a notification lands is exactly when it opens.

The Tauri branch had the mirror race. The desktop page answered "inactive", the
engine allowed the push, and `NativePushRequested` arrived at a window the
reader had just brought forward. The handler dropped the banner because the page
was active, and showed no toast either.

The two-step shape was itself a fix (the sixth iteration in
`system-knowhow/notifications.md`). The toast had fired on the check, and a pong
that missed the deadline gave toast AND push. Moving the toast behind the
decision traded that loud duplicate for this silent loss.

## Decision

**The push is suppressed only on the page's own report that it already
handled the notification.** `PresenceCheck` carries the toast. The page runs the
§4 row matrix in one synchronous step: it shows the toast, auto-reads, or holds
it with toasts off. Then it pongs that outcome. `not_shown` is the only outcome
that leaves the push to the OS. `NotificationToastRequested` is retired.

A Tauri page receiving `NativePushRequested` runs the same step: the banner if
its shell is not in front, else the in-app outcome.

## Rationale

**The marker follows the consumer's acknowledgement, not the producer's
forward.** The old pong was a promise about a toast the engine had not sent yet.
Nothing tied the promise to the later act, so the page could break it by hiding.
Now the pong describes something already on screen. No later page state can
make it false, so the bad interleaving cannot happen at all, rather than being
detected and recovered.

**The remaining failure is loud.** A suppressing pong that misses the deadline
still gives toast AND push, as before the sixth iteration. But the deadline has
since been sized for the slow radio band (`DEADLINE_MS` = 2000 covers the
1100-1800 ms first packet over Tailscale). That makes the duplicate rare, and a
duplicate is a surface the reader sees. The engine logs every late suppressing
pong, so it shows in the log as well.

**One predicate.** The pong read `shellIsActive()` and the toast read
`isPageActive()`, so a page under a fullscreen app could disagree with itself.
One outcome computed at render removes the second reading.

**The desktop frame always surfaces.** The page answered from behind other
windows, so the engine was right to allow the push. The reader moving the window
forward afterwards changes which surface fits, not whether one is owed.

## Consequences

What we keep:

- The reader always gets at least one surface for a notification a page saw.
- The engine wakes at the first suppressing pong, so the common "toast, no
  push" path waits for nobody else.
- One fewer SSE event, and a pong reduced to `{ device_id, outcome }`. The
  dead `focused_thread_id` and `event_in_viewport` fields went with it.

What we give up:

- **A pong later than the deadline means a duplicate.** The toast is on
  screen and the push went out. Loud, logged and rare.
- **The page drops a check past `deadline_ms` by its own clock.** A phone whose
  clock runs ahead of the engine's by more than the deadline answers nothing and
  gets the push. That is the safe direction, never a lost notification.
- **An older bundle's pong is refused** (no `outcome`), which counts as no
  pong. Such a page gets the push until it reloads.

## Alternatives considered

**Keep the follow-up frame and render it even on a hidden page.** It closes the
browser window with no wire change. Rejected: a toast drawn into a hidden page
waits there, the push is suppressed, and on a phone the reader meets it only on
return. That is no interruption at all, which is "neither" in practice. It also
leaves the Tauri mirror open.

**A two-phase claim.** The page pongs "I will show", the engine replies
"accepted" while its slot is open, and the page renders on that reply. It makes
both the duplicate and the loss impossible on paper. Rejected: a lost reply
suppresses the push and drops the toast, the silent failure this change exists
to remove. It also needs replies routed back through the SharedWorker to the
right document.

**Withdraw the toast when the pong was late.** The pong response would say "too
late" and the page would take its toast down. Rejected as detect-and-recover:
the toast flashes, and the code exists only to undo a state that should not
have been reached.

**Retry or re-emit the toast frame.** Any timer or re-queue only narrows the
window. The page can still hide after the last attempt.
