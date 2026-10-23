# 0337: Only the reader's finger moves the mobile dynamic bars; a reply's automatic scrolling holds them

- **Status**: Accepted
- **Date**: 2026-09-30

## Context

The dynamic bars plan (`docs/plans/2026-09-30-mobile-dynamic-bars.md`) settled
that the standing follow's *carry write* counts as the reader's own scroll. A
reply streaming under a rider therefore sent the bars away, and a thread going
quiet brought them back. ADR 0336 kept both when the bars began to glide.

On a phone it read as the app hiding its own chrome. A growing thinking row
pushed both bars away before any answer appeared. A short reply hid them and
brought them back seconds later. The user called it "pretty weird".

## Decision

Only the reader's own scroll moves the bars. A carry write holds them exactly
where they are, as an anchor write already does, and a thread going quiet
reveals nothing. The prompt shows within its height of the thread's end,
whoever put the reader there.

## Rationale

This is how the platform the bars imitate behaves:

- **iOS Safari** collapses and restores its toolbar only on a real touch
  scroll. A programmatic `scrollTo` leaves it alone, and WebKit calls that by
  design.
- **UIKit's `hidesBarsOnSwipe`** runs on a pan gesture recognizer. Scrolling
  done in code never hides the bars.
- **Chat apps** keep their header and composer fixed while a reply streams.

A bar that answers only the finger is predictable: the reader always knows why
it moved. A carry is the app scrolling for the reader. Moving the chrome for it
means the chrome moves while nobody touches the screen.

## Consequences

- `countsAsReaderScroll` counts no write of ours. The hook's hold gate spares
  carry writes as well as anchor writes. It re-takes the baseline, so the next
  finger delta starts where the carry left the reader.
- A carry announces itself at the write through `onRebasedScroll`, as an anchor
  write does. A heavy render can delay its scroll event past the 64ms window.
  The kind check alone would then hand the header the whole jump.
- `markCarryScroll` stamps `carry` on a quiet thread too. A `held` stamp there
  would reveal the bars on a late resize. The same-frame rule stays, so a carry
  sharing a placement's scroll event still lets that event reveal.
- `rideCarriesReader` and the quiet-thread reveal are gone.
- Every reveal that answers a tap or a discrete event stays:
  - the follow toggle's glide, a resume and the submit landing;
  - a chevron, a deep link and a pane swipe;
  - the keyboard closing, focusing the prompt, and a change resolving.
- ADR 0336's glide, edges and gating are unchanged. Only its carry and quiet
  reveal are replaced.

## Alternatives considered

- **Keep the prompt shown during a reply, and let the header go.** Half the
  surprise stays: the header still moves with nobody touching the screen.
- **Hide only once a reply has pushed about a screen of content.** It spares
  thinking rows and short answers, but adds a rule the reader cannot see, so
  the bars still move for reasons they cannot predict.
- **Keep the reveal when a reply finishes, as a cue that it is your turn.**
  Offered and not taken. With the carry holding the bars, the reveal would be
  the one remaining move the reader did not cause.
