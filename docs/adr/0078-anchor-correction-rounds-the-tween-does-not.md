# 0078: The scroll-anchor correction rounds its write to a whole pixel; the tween deliberately does not

- **Status**: Accepted
- **Date**: 2026-08-13

## Context

Two things write `scrollTop` on the chat transcript, and they want opposite
treatment of the fraction.

The **tween** (`animateScroll`, ADR 0065) writes fractionally and never rounds.
On a 2x or 3x display the sub-pixel position is what makes its slow final
approach read as smooth instead of stepping whole pixels.

The **anchor correction** (`withScrollAnchor` in
`components/chat/CreateThreadView.tsx`) is a single write made while the
container is frozen. It holds the reader on the turn they were looking at
across a DOM mutation. It has no frames and no curve: it hands the container
one offset and is done.

Reading ADR 0065's "fractionally, never rounded" as a repo-wide rule is what
this entry exists to stop.

## Decision

The anchor correction rounds its target to a whole pixel before writing
(`reachableScrollTop`). It measures the anchor's position to the SUBPIXEL
against the container's own rect (`contentOffsetTop`) rather than reading
`offsetTop`.

The tween keeps writing fractions, unchanged.

## Rationale

**Layout is fractional and a scroll offset is not.** The container quantises a
fractional `scrollTop` anyway. A residual under one pixel is therefore the floor
for any anchor correction, and no arithmetic removes it. What the rounding
removes is the part that is ours.

**The two engines quantise differently, and one is much worse.** Chromium rounds
a fractional write to the nearest pixel, so it lands within half a pixel. WebKit
TRUNCATES, so handing it x.8 loses the whole 0.8. Rounding first lands both
engines within half a pixel of the same place. Otherwise iOS takes up to twice
the desktop's error for the same press.

Measured on a seeded transcript at a 105% root: WebKit stored 2377 for a written
2377.8 and the reader moved 0.8px; Chromium stored 2499 for 2498.8. Neither kept
the fraction, at either device pixel ratio. Rounding first took the worst press
in that run from 0.75px to 0.39px.

**`offsetTop` cannot be the measurement.** The platform rounds it to a whole CSS
pixel. A correction derived from two of them is wrong by the difference of the
two roundings. That difference is under a pixel, is not zero, and flips sign
between the toggle's two states.

The reader sees the transcript twitch one way on the press and the other way on
the press back. Every line of text re-lands on a different device-pixel row,
which reads as the spacing changing rather than as a scroll.

It bites whenever the layout above the anchor changes by a fractional number of
pixels. That is the ordinary case at any root font size that is not a whole
number of pixels. The mobile default is 112.5%, and every rem-authored height
under it is a fraction. It cannot be seen on a transcript that does not
overflow, because there is no scroll offset to be wrong about.

**Measured against the container's own rect, not the offset parent.** That
answers the same question `offsetTop` does, a distance from the top of the
scrolled content, while surviving two things `offsetTop` has no opinion about:
the container's box moving between the two reads, and the browser clamping
`scrollTop` mid-mutation when the content shrinks. Both rects are taken in one
call, so any transform on the container or above it cancels out.

**The two decisions do not conflict, because the shapes differ.** A tween owns a
curve across many frames, where the fraction is the difference between smooth
and stepped. The anchor correction is one write with no successor, where the
fraction is only an engine-dependent error term.

## Consequences

- A rounded target made the clamp deficit measurable, which is what the debt a
  reveal carried to the next press was built on. ADR 0147 deleted that debt, so
  nothing measures a deficit now and the rounding rests on the quantisation
  argument above alone.
- The clamp deficit was DERIVED from the container's extent, never read back
  from the write. Reading `scrollTop` in the write's own task does not reliably
  answer the new value. The deficit measured that way was intermittent, and the
  reverse press paid out a debt nobody owed. Kept as history: the read-back trap
  is a property of the platform, not of the debt.
- The correction still carries a sub-pixel residual, bounded at half a pixel on
  both engines. That is the floor, not a defect.
- `scrollTop` is a double on the way in and out, which makes the rounding look
  unnecessary at the type level. It is not: neither engine stores what it was
  handed.
- Re-open this only with a measurement showing an engine that stores the
  fraction it was given.

## Alternatives considered

**Write the fraction and let each engine quantise.** Tried and measured worse.
It is what produced the 0.8px WebKit press above, and it makes iOS drift at
roughly twice the desktop's rate for the same interaction.

**Use `offsetTop` for both reads and subtract.** The original shape. Its two
roundings differ by a sub-pixel amount that flips sign between the toggle's
states, which is the visible twitch this replaced.

**Round in the tween as well, for one rule.** Rejected: the tween's whole
smoothness argument (ADR 0065) is the sub-pixel approach, and rounding it
reintroduces the stepping that exponential smoothing's tail was rejected for.

**Snap the layout to whole pixels instead.** Would require abandoning
rem-authored heights or a whole-pixel root font size, which the UI-scale setting
exists to vary.
