# 0209: A live call is green; red is the hang-up the pointer promises

- **Status**: Accepted
- **Date**: 2026-09-17

## Context

The *call toggle* used to turn red the moment a call went live. The reasoning
was that red is a hang-up on every phone, so the on-state should say what the
next press does. The connect before it was green.

Watched end to end, that reads as a failure. The control pulses green while the
call is placed, then flips to red the instant it succeeds. Red is this palette's
destructive tone: `.action-btn-danger` is Delete, Discard and Remove, and the
progress dots use it for a failed step. So the one moment everything went right
looked like the one moment something broke. The owner reported exactly that.

## Decision

Two channels, and each answers a different question.

**Colour says what is happening.** A call that is up is green for its whole
life: `connecting`, `listening` and `speaking` all sit on the same
`--accent-green` paint, and `speaking` deepens the fill rather than changing
hue. `ending` drains to `--text-muted`, and the dwelt connect takes
`--accent-notable`, the waiting tone.

**The pointer says what a press does.** Hovering hands back `--accent-red`, in
every phase where a press ends or cancels the call. `ending` is the exception
and keeps its spent paint, because a press there does nothing.

The shared hover rule is declared after every phase's own paint. They carry the
same specificity, so source order is what makes the promise hold in all of them.

## Rationale

A status colour and an action colour are different things, and this control was
using one channel for both. Splitting them lets each be unambiguous: green means
a call is up, red means this press ends it, and neither has to be read as the
other.

Green for a live call is also what every presence light does. Nothing else in
the app paints a healthy ongoing thing red, and the palette's own semantics put
red on destruction.

The hover is the right home for the action colour, because it is the moment the
reader is asking. The pointer is on the control, so the press is the question,
and the hang-up colour answers it rather than raising an alarm.

## Consequences

Touch has no hover, so a phone reader never sees the red. That is accepted. The
accessible name says "End the call" already, the tooltip says it on a long
press, and nobody needs warning about a call they started.

A future change that paints any resting phase red re-opens the bug. A test
fails on `--accent-red` in any non-hover call-toggle rule, and another fails if
a hover stops offering it.

## Alternatives considered

**Red for the whole call** (the other consistent option). It removes the flip
just as well, by making the connect red too, since a press there cancels.
Rejected because it paints a healthy call in the destructive tone for minutes at
a time. That is the half of the complaint which is not about the transition.

**Keep the flip and soften the red.** A muted red for live, the full red for
hover. Rejected as the worst of both: still a hue change on success, and a
weaker hang-up promise.

**Colour by phase, with a legend nobody reads.** Green connecting, blue live,
red ending. Rejected because a four-colour control in a row of five icons is a
light show. That is the same reason the stylesheet gives for refusing a badge
inside the button.
