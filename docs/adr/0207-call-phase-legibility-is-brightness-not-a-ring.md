# 0207: The call toggle tells its phases apart by brightness, never by a ring

- **Status**: Accepted
- **Date**: 2026-09-17

## Context

The *call toggle* wears the *call phase*: green while `connecting`, red while
live, deeper red while `speaking`, muted grey while `ending`. Two of those are
work rather than a steady mode, so they move. Colour alone cannot carry them. A
reader who cannot separate green from grey loses the connect against the
hang-up, and `prefers-reduced-motion` takes the movement away entirely.

The first two answers both reached for a shape. An arc swept the control while
it connected, then a whole circle pulsed out of it. The owner turned each down
on sight and asked for the control itself to pulse instead.

## Decision

Nothing is drawn around the handset. The glyph carries the phase, and each
phase rests at a brightness of its own: the connect at 0.7, the hang-up at
0.55, the dwelt connect and both live phases at full.

`styles/__tests__/call-toggle-phase-paint.test.ts` holds both halves. One test
fails on any pseudo-element under a `call-toggle` selector. Another fails if a
transitional phase stops declaring a resting opacity, or declares the same one
as its neighbour.

## Rationale

Brightness is a non-colour cue that costs no geometry. It reads for a colour
blind reader, it survives a stopped animation, and it says something true:
a control part-lit is not a control that is up.

A ring reads as a spinner at this size. The button is a 2.25rem box holding a
1.25rem glyph, so a circle inside it clears the handset by a few pixels. The eye
then takes the whole thing for a loading indicator wrapped round a phone. That
is what the owner rejected, twice, and pulsing outward rather than spinning did
not change the read.

## Consequences

The connect and a live call differ by hue plus brightness, where they used to
differ by hue plus a drawn mark. That is a thinner distinction and it is the
price of the decision, paid deliberately.

A future reviewer will reach the accessibility argument alone and propose a
badge, a dot or a ring, because it is the obvious answer. It has been proposed
and refused. Read this entry before re-opening it, and take the question to the
owner rather than to the stylesheet.

## Alternatives considered

**A sweeping arc** (shipped first). A `::after` with one coloured border side,
rotating. Rejected on sight: a spinner squeezed in beside the glyph.

**A whole ring pulsing outward** (shipped second). The same pseudo-element as a
complete circle, scaling out and fading, resting closed so reduced motion saw a
deliberate outline. Rejected the same way, with "no circle and definitely no
dotted circle". The dotted half was the dwelt connect, which used a dashed
border to say it had stopped claiming progress.

**A dot or badge inside the button.** Never shipped. It loses for the reason the
stylesheet already gives for `speaking`: this is one control in a row of five,
and a mark appearing inside it reads as a light show.

**Colour alone.** The cheapest option and the one this entry exists to refuse.
With reduced motion on it collapses the connect into a live call, and for a
green-grey reader it collapses the connect into the hang-up.
