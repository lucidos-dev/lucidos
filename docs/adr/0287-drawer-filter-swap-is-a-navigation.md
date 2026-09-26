# 0287: The Threads/Filters swap and every pane title move like a page navigation

- **Status**: Accepted
- **Date**: 2026-09-26

## Context

ADR 0276 put every Filter change on one timing, `--duration-fast`, as
transitions on elements that stay mounted. The panel faded through the pane
background, and the pane title crossfaded between "Threads" and "Filters".

The user still found the swap fast, janky and weird. Its two directions used
different shapes: opening faded the options in, closing faded a veil out over
the list. And a content-pane page navigation, the other view swap in the app,
moved differently again: an opaque cover clearing on `--duration-normal
ease-out`.

## Decision

The Threads/Filters swap is a navigation of the drawer pane. It uses the same
navigation cover as the content pane (`components/shared/NavigationCover.tsx`,
`.nav-cover`): the leaving view goes at once, and the cover clears off the
arriving one. The filter cover only shows or hides.

Every pane title arrives with its view: the drawer's and the content pane's. A
title keyed on the pane's view key replays `.nav-arrive`, the cover's mirror, on
the same curve. One hook, `useArrivingView`, drives the cover and the titles.

The Filter button's glyph, badge and pressed highlight stay on
`--duration-fast` transitions, like every header icon.

## Rationale

One view swap should look like every other view swap. The cover already solved
the hard part: it hides the swap frame, and it never waits on the view.

A keyed element replaying an animation is the reliable shape here. A
class-toggled transition needs its start state on screen before the end state
lands, and silently hard-cuts when it loses that race.

0276 kept every fade on an existing element because WebKit can start an
animation on a fresh element a frame late. That no longer splits the view from
its title: both are fresh, so both start together. And each rests in its start
state, the cover opaque and the title transparent. A late first frame holds the
start rather than flashing the end.

## Consequences

- Open and close have one shape, on the page-navigation curve.
- A swap during a fade restarts from the opaque cover, as a page navigation
  does. It no longer reverses from where it is.
- The content pane title fades in on every navigation, not only the drawer's.
- The title no longer reserves the wider word's box. It is centred, so a
  shorter word only changes its own width.
- 0276's "one timing for every Filter fade" now holds for the button only.

## Alternatives considered

**Retime the old transitions to `--duration-normal ease-out`.** Cheap, but the
two directions keep their different shapes, and the class-toggled transition
keeps its start-frame race.

**A `CrossfadeStack` variant for the titles.** It fits "Threads"/"Filters", a
fixed pair. The content title is open-ended, and a stack needs every layer
mounted, so the two titles would have used two mechanisms.

**Cover the header title with its own veil.** The header has its own background
and chrome, and a veil there would cover the chevrons too. Fading the title
itself touches nothing else.
