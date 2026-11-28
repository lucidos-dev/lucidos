# 0713: The follow toggle slides left of the down chevron while it shows, in one row; supersedes ADR 0596's vertical stack

- **Status**: Accepted
- **Date**: 2026-11-16

## Context

ADR 0596 put the follow toggle (the magnet) over the composer's top edge and
stood the down chevron one slot above it. The maintainer asked for the magnet
to sit left of the chevron instead, sliding left when the chevron shows and
back right when it hides.

ADR 0596 rejected "the two side by side" and gave "the magnet never moves" as
its first reason. This ADR replaces that placement. Everything else in 0596
stands: the magnet lives in `.prompt-area`, is always shown, and wears the
chevrons' round surface.

## Decision

The down chevron takes the magnet's row, at the right edge. While the chevron
shows, the magnet slides left by one slot plus the stack gap. When the chevron
hides, the magnet slides back into the right-edge slot.

## Rationale

- **One row costs no height.** The stack put the chevron a full slot above the
  composer, over the last lines of the transcript. In the row it covers no more
  of the transcript than the magnet already does.
- **The motion says what happened.** The magnet steps aside for the jump and
  returns once the reader is back at the bottom.

## Consequences

- The magnet moves, which 0596 avoided. Once the chevron lands the reader at
  the bottom, the magnet glides back under the pointer. A late second tap on
  the chevron's spot then toggles follow. The maintainer accepted that trade.
- The slide is a `transform`, because the phone's dynamic-bars glide already
  owns `translate` on the magnet.
- On touch the two reaches face each other horizontally: the chevron reaches
  half the gap to its left, the side-by-side magnet half the gap to its right.
- `--scroll-chevron-stack-gap` now spaces the row, not a stack.

## Alternatives considered

**Keep ADR 0596's stack.** The magnet never moves, but the chevron sits one slot
higher, over the transcript. The maintainer preferred the row.

**Fixed side by side, the magnet always one slot in.** Nothing moves, but the
magnet leaves the right edge empty whenever the chevron hides, which is most of
the time.
