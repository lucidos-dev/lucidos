# 0245: A keyboard close is seen without an event, and names its path

- **Status**: Accepted
- **Date**: 2026-09-22

## Context

The nineteenth report of the dead composer on the iOS PWA. The user typed an
answer, tapped Submit, and nothing happened. The reconstruction is
[`docs/plans/2026-09-22-the-keyboard-close-is-seen-without-an-event.md`](../plans/2026-09-22-the-keyboard-close-is-seen-without-an-event.md).

[ADR 0228](0228-composer-recovers-on-the-keyboard-close.md) made the keyboard
close the trigger for the relayout. Three readings say that trigger did not
fire.

**The reported close was never detected.** The draft writes put the last
keystroke four seconds before the press that finally worked. That press carries
`keyboardActive: false` and a full-height viewport, so the keys went in those
four seconds. It also carries `quiet.closes: 0`.

**Only one caller can see a close.** `noteViewportResize` is called from the
app's `visualViewport` resize handler and nowhere else. `onWake` and
`onOrientationChange` both restore `--app-height` without folding a reading.
`onWake`'s own comment already recorded the hole: iOS dismisses the keyboard on
resume and fires no resize.

**Timers keep running while touches do not.** A later episode the same hour took
nothing for 36.4 seconds while twelve scheduled checks ran through it. So a
polled reading can see a transition no event delivered.

## Decision

**A close is whatever any of three paths observes, and every line says which
one.** `resize` stays the first responder. `wake` is a resume, where no resize
fires at all. `poll` is a scheduled reading finding a transition nothing
announced.

**A resume has two halves, and both reach the fold.** iOS sometimes leaves the
height pinned at the shrunk value and sometimes returns it corrected. A pinned
one offers no edge, so the stamp goes on the handler's word. A corrected one
carries the restored reading the fold has waited for. Handing it over is the
only way that close is ever seen.

**All three live with the height owner**, the `visualViewport` effect in
`MobileSwipeContainer`. The poll runs the same three steps `onResize` does, in
the same order. A poll-seen close is one no resize settled the height for. So a
poll parked anywhere else would bounce off the keyboard-shrunk `--app-height`
and restore it, leaving the shell at half height.

**The poll rides the Perf instrumentation toggle, and the wake hook does not.**
The wake path costs nothing and closes a documented hole, so it ships to
everyone. The poll costs an interval, so it runs only on a device that asked for
diagnostics.

**A cover reading retires the close.** `silent-since-keyboard` named a keyboard
that had come back up a minute earlier, contradicting the viewport printed
beside it.

**The wake path is an edge too, and its echo suppression is time-bounded.** One
resume fires `visibilitychange` AND `pageshow`, so the hook runs twice for it.
iOS leaves the height pinned briefly after a resume, and a covered reading
inside that window is dropped as the echo. The same window scopes the paired
event, because both halves of it land inside the window and a later resume does
not.

**Both halves of the resume open that window.** iOS can hand the two events
different viewports, so a window only the pinned half opened leaves
corrected-then-pinned unguarded. That is two closes for one resume.

**`keyboardCloseRelayout.ts` still imports nothing.** It knows nothing of the
gate, which lives entirely with the caller.

**Nothing is pressed, on any path.**
[ADR 0225](0225-composer-never-sends-by-itself.md) is unchanged.

## Rationale

The poll is the reading this investigation has never had. Every other signal the
page can take arrives as an event, so a silent ledger has meant two opposite
things at once: the platform delivered nothing, or nothing happened. Round 12
named that blind spot and round 18 narrowed it, and both stayed inside the event
channel.

A timer is outside it. The ledger establishes that timers run through a wedge,
because the scheduled checks kept reporting while the page took no touch. So a
close the poll records and the resize does not is direct evidence that WKWebView
stopped delivering to the page. That is the mechanism nineteen rounds have
failed to name, and no reading taken from the layout side can supply it.

Gating the poll is a real cost, weighed and accepted. The recovery it adds
reaches only a device with diagnostics on, so the wake hook carries the ungated
half. Every reported episode so far has come from one phone, and that phone can
carry the toggle.

## Consequences

- **The recovery answers a close on any of the three paths**, so a resume that
  fires no resize now spends its relayout.
- **`quiet.closes` counts more closes than before**, because the wake path was
  invisible to it. A ledger read across this change must not compare the
  counts.
- **`silent-since-keyboard` can no longer cite a stale close.** A silence after
  the keys return writes no line at all. That is correct, and it is one line
  fewer than the same sequence used to produce.
- **`KeyboardCloseState` gained a field**, so a caller destructuring it whole
  sees `path`.
- **A phone with Perf instrumentation on runs one more interval**, reading two
  viewport numbers every three seconds.
- **The next episode says whether the resize event arrived**, which is the only
  thing this round is really for.

## Amendment: the poll answered, and the answer is no

The twentieth report came hours after this shipped, with Perf instrumentation
on, so the poll was running through it. The composer took no touch for 16.5
seconds. The close was seen anyway, and the ledger names it `closePath:
resize`. No `poll` close has been recorded at all.

So the resize pipeline was alive while the touch pipeline was dead. The
Rationale above reads a `poll` close as evidence that WKWebView stopped
delivering to the page. Its absence carries the sharper finding: delivery did
not stop, and the fault is specific to touch.

The poll stays. One negative episode rules out the general claim, and not the
chance that a later wedge starves the page outright. The field is what tells
the two apart. The round-20 plan is
[`docs/plans/2026-09-22-the-touch-pipeline-is-the-one-that-dies.md`](../plans/2026-09-22-the-touch-pipeline-is-the-one-that-dies.md).

## Alternatives considered

**Hand the wake path a viewport sample instead of a verdict, always.**
Rejected, and the reason is why the resume is split in two. iOS leaves
`visualViewport.height` pinned at the shrunk value across many resumes, which
is why `onWake` trusts `window.innerHeight` in the first place. Folding a
pinned sample reads a cover, not a close, and arms a second close on the
correction. Only the corrected half is safe to fold, and taking the verdict for
both loses every close on that half.

**Poll always, with no gate.** Weighed, and it is the variant that makes the
next episode readable wherever it happens. Declined on the cost of a permanent
interval on every mobile client, which buys nothing on a phone nobody is
debugging.

**A dedicated toggle beside Perf instrumentation.** Rejected as a second switch
to remember and a second flag to carry, for a device-local diagnostic whose
existing switch already means "instrument this phone".

**Put the poll in `keyboardCloseRelayout.ts`, behind an injected gate.** Built
that way first, and rejected in review. The module owns no height, so its
relayout restores whatever `--app-height` already said, which on the poll's own
path is the stale keyboard-shrunk value. Parking the poll with the height owner
answers that, and drops the gate from the leaf entirely.

It does NOT answer the third objection raised beside those two. The
gated-interval lifecycle, a start and stop pair with a gate read inside the
tick, is a second copy of `utils/mainThreadStall.ts`. Moving it did not remove
it. A shared helper is the right cleanup, and it means converting the stall
probe too. Its own `visibilitychange` re-anchor does not fit the generic shape,
so that is a separate change rather than a rider on this one.

**Suppress the post-wake echo until a reading clears it, with no clock.** Built
that way first, and rejected in review. A genuine keyboard reopen IS a covered
reading, so the suppression never lifted and the real close after it was lost.
Losing a close is the failure the module exists to prevent, and over-counting
one costs a spare invisible relayout, so the bound errs short.

**Guard the repeated wake on the outstanding stamp alone, with no window.**
Built that way first, and rejected in review. Nothing retires a stamp except a
reading, and the wedge is the state where no reading arrives. The guard
therefore swallowed every later resume. A user picking the phone up again got
no relayout, and the ledger dated their silence from the first resume.

**Put the poll in `deadPressProbe.ts`, where a scheduled check already runs.**
Rejected on ADR 0228's own ground. That module is a diagnostic due for deletion,
and the recovery must outlive it.

**Widen the touch box, or otherwise treat this as aim.** Rejected, as it has
been every round since the twelfth. The composer is dead anywhere while it is
wedged.
