# 0768: The transcript scroll module moves the reader only for a ride they asked for

- **Status**: Accepted
- **Date**: 2026-11-22
- **Related**: [0064](0064-transcript-scroll-belongs-to-the-reader.md), [0080](0080-submit-rests-at-the-live-edge.md)

## Context

`scrollState.ts` decides when the app moves the transcript under the reader. A
streaming reply, a decoded image or a growing composer can change the geometry
at any moment. The module must tell the reader's own acts from the platform's
and from content growth. It also stays light, so it must not import `store`.

This record restates the design comments of
`crates/lucidos-app/src/components/chat/scrollState.ts`, written before the
comment strip removes them. Nobody checked it line by line against the code.

## Decision

- A resize moves the reader for exactly four reasons. They asked to ride the
  live edge, handled by `keepTheLiveEdge`. They just submitted and the turn
  their landing waits for has rendered. Content moved what they just arrived
  at, handled by `holdTheArrival`. The pane's width changed and rewrapped the
  content above them, handled by `restoreAfterReflow`, which holds their anchor.
- The live edge and the reflow correction are alternatives, never a sequence.
  Neither writes for a rider whose wheel or key scroll is still owed its event.
- For everyone else a resize moves nobody. A streaming reply, a decoded image,
  an expanded step and a growing composer all leave the transcript where it is.
- The follow seed is the reader's last press of the toggle, kept across threads
  and reloads, and armed until a disarm press. Only a thread with no reading
  position of its own starts from it.
- Only the toggle writes the follow seed. A scroll that retires the follow
  records itself as that thread's offset. The seed is device-scoped.
- Our own held writes are recorded as the ledger's hold, `holdPosition`. The
  reader's gesture is told from our write by position, not by the event window.
- `makeScrollObservers` owns the reader's input wiring. `attachReaderInput` is
  not exported and the view never calls it.
- A deferred submit that creates a turn waits with a baseline of zero rows. A
  deferred card submit waits with a baseline of what the turn already holds. A
  card submit waits for the engine to confirm it.
- There is one at-the-live-edge threshold, `isAtLiveEdge`, and it takes an
  offset. There is one scrollable predicate, shared with `threadWindow`. The
  down chevron asks `isAtLiveEdge`'s 2px instead.
- A deep link's rest offset has one definition, a per-container closure. The
  link scrolls to it and decides the ride's fate by it.
- Two subscriber lists tell `attachScrollMemory` about a deep link. One fires
  when a link takes the claim, the other when it finds its target. Neither
  fires on release.
- The caller supplies the dead-link message, and the module stays free of the
  heavy `store` import.
- The question card a send would answer is read from the DOM. Only the newest
  card is asked, and the dead states are named positively from the rendered
  classes.

## Rationale

- Every move of the reader should trace to something they asked for. Growth
  below the reader changes `scrollHeight` and never `scrollTop`, so position
  tells their gesture from ours exactly.
- The follow seed stays out of threads the reader has parked in, in the same
  way `selectedScope` does for the destination picker. A scroll in one thread
  must not cancel a standing preference for every future thread.
- Following is a property of the screen in front of the reader, not the
  account. So it is right on first paint with no server round trip, and the
  toggle can be armed before the first send.
- A new turn did not exist at submit, so every row in it is the agent's later
  work and the first row ends the hold. A card's turn was on screen, so its
  existing rows predate the answer. A zero baseline there would end the hold at
  once.
- One definition per question keeps callers from disagreeing. A deep link must
  not scroll to one number and judge the ride by another.
- Owning the input wiring in `makeScrollObservers` makes the state of observers
  attached with input forgotten impossible to express. That state is silent
  until a reader scrolls away from a live reply and is dragged back.
- Keeping `store` out lets lean importers such as `promptFocus` avoid its
  module-load side effects.

## Consequences

- A send made while a question card's divider is collapsed finds no card,
  because the body is unmounted. It takes the send's own landing, and nothing
  here can recover the card without a marker that survives the fold.
- A dead link leaves the transcript where it was, and the toast words are the
  whole recovery.
- A deep link's landing is reported by an explicit found notice, not inferred
  from the container moving. Arriving in a shorter thread can clamp the
  container to the same offset, which would otherwise read as a dead link.
- A target beyond the live edge answers true for the edge threshold, since the
  browser clamps it back to the edge.
- The store-touching orchestration for navigated turns lives in
  `hooks/useKeyboardShortcuts.ts`, not here.

## Alternatives considered

- Counting the rows a turn holds when it first becomes addressable was the bug
  the zero baseline replaced. Preact has already committed the transcript, so a
  turn arriving with the agent's opening read as one that drew nothing. The
  hold ran on until the reply landed and took the reader with it.
- A zero baseline for a card submit is rejected, because it would read the
  agent's earlier rows as the answer's reply.
- Wiring the observers and the reader input separately is rejected, because it
  makes the forgotten-input state expressible.
- A second body of the scrollable predicate in the chevron is rejected. The render-window fill could stop short of a transcript the chevron calls unscrollable.
- Asking `hasPendingEventScroll` once at attach is rejected. A link into the
  current thread re-attaches nothing, and a thread whose events arrive mid-tap
  attaches before the claim. The restore also cannot re-ask, because the claim
  is released within a second and the restore stays armed for three.
- Inferring a deep link's landing from the container having moved is rejected,
  because a landing with nowhere to move reads as a dead link.
- Inferring a dead question card from a missing id is rejected, because an id
  that grew onto a dead body would make it answerable again.
- Letting a scroll that retires the follow also write the follow seed is
  rejected.
