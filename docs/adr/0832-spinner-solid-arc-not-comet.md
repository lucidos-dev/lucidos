# 0832: The shared spinner turns a solid arc, not a comet

- **Status**: Accepted
- **Date**: 2026-11-27
- **Amends**: [0827](0827-one-spinner-ring-and-comet.md), the look only.

## Context

ADR 0827 made the shared spinner a still ring with a comet turning on it. The
comet has a bright head and a tail that fades into the ring.

In use it read as a different drawing style. Every icon beside it is a solid
stroke with a hard, round end. The fading tail was the only soft-edged shape in
the interface.

## Decision

The turning layer is a solid arc of 100 degrees with a round cap at each end.
It keeps the ring's stroke and colour. Everything else in 0827 stands.

## Rationale

A spinner sits beside icons in a row, a button and a header. It must share
their stroke, their line ends and their flat colour to read as one set.

The arc keeps what made 0827 work. The ring stays still, only one layer turns,
and both layers are gradients behind one mask.

## Consequences

- The arc layer is one flat colour behind a mask: the ring, intersected with
  the arc and its two caps. A translucent `--spinner-color` therefore stays
  even where a cap overlaps the arc.
- The spinner now needs `mask-composite`, or its `-webkit-` form. A browser
  with neither draws the whole ring bright and shows no turn.
- The far cap's position uses the arc's sine and cosine, written as numbers. A
  test derives them from the angle, so they cannot drift apart.
- The numbers are literals, not `sin()` and `cos()`. A WebKit without those
  functions would drop the whole mask.
- Apps that use `.mini-spinner` get the arc with no change.

## Alternatives considered

- **Keep the comet.** Rejected by the user after living with it: too smooth
  beside the icons.
- **A breathing arc**, which grows and shrinks as it turns. Busier with several
  in a row, and 0827 already rejected it.
- **A bead on a thin wire ring.** The lightest option, and too hard to see at
  the drawer's 0.5rem.
