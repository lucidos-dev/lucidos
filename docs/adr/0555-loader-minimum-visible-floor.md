# 0555: Every loader the delay gate let through stays opaque for a shared minimum of 500 ms, held by its exit owner. Supersedes the floor clause of ADR 0081.

- **Status**: Accepted
- **Date**: 2026-10-23

## Context

The dashboard's loading skeleton showed as a tiny flash. The dashboard already
used the shared loader: `useDelayedFlag` (a 300 ms delay gate) plus
`<LoadingFade>`. The gate opened at 300 ms, the dashboard settled a few frames
later, and the skeleton faded almost at once. Every loader in the app does this
when its wait lands just past 300 ms.

The rules said "delay-only, no minimum-visible floor" (ADR 0081 and
`.claude/rules/frontend.md`). A floor built in June (`useDelayedMinFlag`) was
removed for two reasons. It was inert, because views chose loader versus content
from the loadable and not from the flag. Where it fired, it withheld content.

Meanwhile the app grew three separate "hold it long enough to read" mechanisms:
none in `<LoadingFade>`, none in the transcript's `ThreadSkeletonOverlay`, and a
private 400 ms floor in `RefreshIndicator`.

## Decision

A loader the delay gate let through stays opaque for at least
`LOADER_MIN_VISIBLE_MS` (500 ms) from the moment it appeared, then fades out as
before. The exit owners hold it, `<LoadingFade>` and `ThreadSkeletonOverlay`,
through `useMinimumVisibleFlag`. Callers keep passing the delay-gated flag.
`RefreshIndicator` reads the same constant. The delay stays 300 ms.

This supersedes the "no floor" clause of ADR 0081 and `frontend.md`. ADR 0081's
main decision stands: nothing holds a skeleton ahead of the gate.

## Rationale

Delay plus minimum is the standard pattern for loaders (the `spin-delay`
library is the common form). The delay keeps fast loads from flashing. The
minimum keeps a loader that did appear from blinking.

Putting the floor inside the exit owners fixes the June failure structurally.
A view cannot bypass it, because the view never decides when the skeleton ends.
While the floor holds, `<LoadingFade>` hides its content (`visibility: hidden`,
layout kept), so loaded content never draws beside an opaque skeleton. The
transcript overlay is already opaque, so the transcript hides under it.

The floor is legibility, not animation. The Animation speed slider and reduced
motion do not scale it, as with the refresh spinner before it.

## Consequences

- A load that ends at 301 ms now shows its content at about 800 ms. A load under
  300 ms still shows no loader and waits for nothing. A load that outlasts 500 ms
  of skeleton ends the skeleton the moment it finishes.
- The floor value is defined once. A second literal for it is a bug.
- `useDelayedFlag` stays floor-free: it also backs non-loader flags (the reload
  offer, the connection banner).
- A load-more `.mini-spinner` row gets no floor. Holding it would keep a spinner
  under rows that already arrived.

## Alternatives considered

**Raise the delay instead.** A longer delay shows fewer loaders. It also makes
every slow load feel dead for longer, and a load landing just past the new delay
still flashes. It moves the flash and does not remove it.

**A floor in each caller.** This is the June design, and it failed because a
view could pick loader versus content from the loadable and ignore the flag.

**A floor inside `useDelayedFlag`.** The hook also backs flags that are not
loaders, and holding those makes no sense.
