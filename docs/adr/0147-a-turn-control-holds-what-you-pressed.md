# 0147: A turn control's scroll anchor is the element the reader clicked

- **Status**: Accepted
- **Date**: 2026-08-28

## Context

A *turn control* changes the height of every turn in the transcript.
`withScrollAnchor` (`components/chat/CreateThreadView.tsx`) writes a scroll
correction across that change, so the press does not move the reader. Which
element it holds still is what decides who gets held.

Until this change the anchor was a RANKING. It preferred the reader's own
topmost visible line. Behind that came the *transcript seam* that line collapses
into, then the panel around it, then the turn. The turn the control sat on came
last.

The user reported the transcript jumping on expand and on collapse, four times.
The fourth report came with screenshots, and they name the mechanism. A short
thread fits on screen with the steps hidden. The reader's topmost line is then
the first turn's first row, at `scrollTop` 0. They pressed the SECOND turn's
control. Holding that first row means holding 0, so the first turn grew a step
list and carried the pressed turn off the bottom.

The ranking came from "fix(chat): the step-log toggle holds the reader from
every position". It was driven by a sweep that parks at 24 offsets and presses
through `btn.click()`, on a control it never scrolls into view. The controls
live in `.response-header`, which is not sticky, and keyboard activation scrolls
its target into view first. So the ranking was tuned for a press only a
synthetic click can make.

## Decision

**The anchor is the element the reader clicked, unconditionally.** The click
handler hands over its own `currentTarget` (`heldOnThePress` in
`ChatExchange.tsx`), so nothing is guessed from a selector.

All four ways in take it: the three turn controls, and the `⋯` stub that unfolds
a folded turn. The ranking and every scan it walked are deleted.

## Rationale

The reader asked for one named thing to change, by pressing it. A turn control
changes heights and nothing else. So the thing they pressed is the thing that
must not move. No other reading of "hold them still" survives a press made from
the top of a short thread.

Guessing the anchor is what turned it into a five-way ranking in the first
place. A rule keyed on the click cannot drift that way, because the click
already names the element. That matters more than it reads: this control has
been re-broken four times, and each fix added a candidate rather than removing
one.

**A detached anchor gets no correction at all.** One press removes its own
target, the `⋯` stub being replaced by the body it reveals. A detached node
answers an all-zero rect, which reads as content at the very top of the thread.
An unfold changes nothing above its own turn, so the freeze has already left the
reader right and there is nothing to write.

**A clamp debt names the element it was measured for.** Collapsing to a
transcript shorter than its pane has to clamp. The deficit is remembered, so the
reverse press lands the reader back where they started. Keyed on the container
alone, that credit was spent by a press of a DIFFERENT control. The reader then
moved by a number describing somebody else's turn. It is the user's own first
observation, the one they withdrew.

> **Amended: the debt is deleted.** It spent one press's clamp on the next
> press of the same control. Hiding the steps at the end of a thread clamps,
> and the reverse press then threw the pressed control up the screen. So the
> mechanism written to serve the decision above was the thing breaking it. A
> press reads only its own two measurements now. The clamped press leaves the
> reader off the live edge, so the reverse press has the slack it needs.

**Only a text field is blurred before the mutation.** iOS scrolls a focused
field clear of the soft keyboard, and that scroll fights the correction.
Blurring any focused element instead takes a keyboard reader out of the
transcript. The press that folds a turn then removes the control that unfolds
it.

## Consequences

- The correction is one subtraction of two readings of one element.
  `readersLine`, `resolveSeam`, `readerTopEdge`, `readerEdgeInset`, the row and
  panel selectors and the sliver constant are all gone.
- *Transcript seam* is retired in `docs/glossary.md`. It existed only to serve a
  reader whose own line the reveal had removed.
- Folding a turn anchors for the first time. `toggleCollapsed` wrote the store
  directly before this.
- The `⋯` stub cannot collect a clamp debt the header control earned, because
  the two are different elements. Keying the debt on the turn would buy that
  round trip and sell a credit shared by two unrelated controls. Moot under the
  amendment above: no press collects a debt at all.
- Folding the LAST turn from a control at the foot of the pane missed by 70px on
  WebKit, and the sweep skipped that one park. Diagnosed a day later as a CLAMP
  rather than an unsettled read: the fold takes its rows out before the stub
  goes in, so the offset is clamped against a container that is briefly tiny.
  The correction re-asserts until the height stops moving now, and the sweep
  covers the park.
- ADR 0064 was left unchanged here, and reversed a day later. Its amendment
  exempted an armed reader measured on the live edge, on the strength of a
  ranking this decision had just deleted. Holding the pressed control cannot
  produce the drift that amendment answered. So the press wins from every park
  now, and only a ride already carrying the reader outranks it.
- A press that collapses the transcript by an order of magnitude reveals the
  mobile header. `useHideOnScroll` re-takes its baseline one frame after the
  anchor write, and a shrink that large settles later. The reader lands near the
  top of a short thread, where the header belongs revealed, and it stops well
  clear of the control. Left as a non-goal, with the numbers, in the plan.

## Alternatives considered

**Hold the pressed turn's header when it is on screen, and fall back to the
ranking otherwise.** Drafted first, then rejected. It keeps the ranking alive
for a case no finger can reach, and unreachable scroll-correction code is how
this control got re-broken four times.

**Keep the ranking and fix the short-thread case.** Rejected. The reported shape
is not special: any press whose turn sits below the reader's own line carries
the control away from them. The ranking is wrong about the general case, not
about one thread.

**Fall back to the pressed element's TURN when the mutation detaches it.**
Rejected as unnecessary. The only press that detaches its own target is the `⋯`
stub, and its turn does not move either. The fallback reaches the same answer
through more code.

**Key the clamp debt on the turn rather than on the element.** Rejected. It buys
the header-then-stub round trip, and sells a credit shared between two unrelated
controls on one turn.

**Keep the clamp debt at all.** Rejected later, per the amendment above. Two
readings of "hold them still" were in play: the control does not move, and the
pair of presses ends where it began. The second buys the round trip by moving
the reader on a press they made, and that breaks the first. A clamp the
geometry forces is visible to the reader; a credit stored against a button is
not.

**Narrow the debt to a transcript shorter than its pane.** Rejected with it. The
target exceeds the reachable maximum in exactly one situation, too little
content below the anchor, and an unscrollable transcript is only its extreme. A
line drawn there would be arithmetic dressed as a rule.
