# 0276: The drawer toggle stays in the header's leading corner

- **Status**: Accepted
- **Date**: 2026-09-24

## Context

The desktop thread drawer toggle had two resting places. With the drawer shut,
it sat in the header's leading corner, after the traffic lights on the packaged
macOS build. With the drawer open, it sat at the drawer's far edge, at the start
of the Conversation pane header. The drawer row's Filter button then held the
corner.

A tester could not find the toggle with the drawer open. They looked in the
corner, where they had last seen it, and found Filter. The toggle's glyph, a
bulleted list, looked like Filter and like the Canvas pane's hamburger. So the
corner looked right while it held the wrong control.

The drawer-open position was never chosen for its own sake. It came from an
older header layout, and the two reworks after it kept both positions so that
nothing would move (`docs/plans/2026-08-09-drawer-toggle-travels-with-the-header.md`).

## Decision

The toggle rests in the header's leading corner in both drawer states: at
`--header-lead-inset`, which is 0.5rem on the web build and clears the traffic
lights on the packaged one. The drawer row keeps that end free for it and clips
there. Filter moves to the row's trailing end, beside Search. On desktop the
toggle's glyph becomes a sidebar icon. On a phone it stays a bulleted list.

Filter's pressed highlight means the filter panel is open, and nothing else.
Its glyph says what the list is filtered to, open or closed: a status view's own
icon, an outline funnel for All statuses, and a filled funnel while thread types
narrow it. While the panel is open the badge drops. It no longer turns into an X.

Filter draws a funnel shape, never a stack of lines.

*Superseded in part 2026-09-26, for the panel and the pane title (ADR 0287):
they now move like a page navigation. The button's fades below still hold.*

Every state change of Filter fades, on one timing, `--duration-fast`: the
glyph, its badge, the pressed highlight, the pane title and the filter panel.
The glyph and the title crossfade. The panel fades THROUGH the pane
background: the arriving view fades in and the leaving one goes at once, both
ways. Opening hides the list and fades the cover in with its options. Closing
hides the options and fades the cover out over the list. Nothing scales,
bounces or slides.

## Rationale

A control that moves has to be found again each time; one that stays put is
found once. The corner is also where desktop apps put this button, and on macOS
that means right after the traffic lights.

Moving Filter to the trailing end costs the drawer's floor nothing up to about
166% UI scale. The title is centred on the pane, so the floor pays the wider of
the row's two ends twice (ADR 0058). The leading end already carries the lights
reserve and one icon. Filter and Search together are narrower than that below
that scale.

The clip is what makes a pinned toggle safe to animate. As the drawer shuts,
Filter and Search ride the row's shrinking right edge into the corner. The clip
ends them at the toggle's edge instead of sliding them under it.

A sidebar glyph describes the desktop drawer, a column beside the conversation.
On a phone the toggle opens the threads pane, which fills the screen, so there
the glyph draws the list it opens.

That list shares a corner with Filter. On a phone the threads pane puts Filter
where the thread pane beside it puts the toggle. Drawn as three lines, Filter
read as the way back, the same mix-up this record began with. A funnel shape
shares nothing with a list.

Narrowed shows as a filled funnel, not a dot. A dot beside the spout read as
part of the funnel on a phone. The other usual place, the top-right corner, is
where the needs-attention badge sits. Outline for off and filled for on is also
the iOS convention.

An X at the far end of the header reads as "close this pane", not "close the
filters". Pressed, the button keeps its glyph, and with it its identity, and
still says the filter panel is open.

The highlight first meant "a filter is on" as well as "the panel is open". A
filtered list then looked pressed all the time, and a pressed button could not
say whether the panel was up. One meaning per cue fixes both: the highlight
answers "is the panel open", and the glyph answers "what am I looking at".

One timing makes the button, its badge, the title and the panel read as one
change. A fade suits a state change: it says something is different without
calling for attention.

The panel copies the list's geometry on purpose, so a crossfade would print
rows and options on the same lines for the middle frames. The fade through
shows one or the other on every frame. There is no slide either: Filters is a
view swapped in place, not a step deeper, and on a phone the pane already
slides.

Every fade runs on an element that already exists when the state changes.
WebKit can start an animation on a freshly mounted element a frame late, and
that frame would split the fades apart. So the glyph keeps all six shapes
mounted, the badge stays mounted while hidden, and the panel's cover and
options are always there. The open signal still drives Escape, the overlay
stack and the pressed state at once.

The header paints resting icons in a translucent white. Two translucent shapes
stacked mid-crossfade paint their overlap twice, which is the bright rim the
filled funnel was redrawn to remove. So the glyph's wrapper carries the
translucency, and the shapes inside paint opaque.

## Consequences

- Filter sits on the right of the desktop drawer row and stays on the left of
  the mobile threads row. The phone row keeps two icon slots per end. A Filter
  on the right would touch the Lucidos mark there, and overlap it above about
  160% UI scale.
- The drawer's search field starts after the toggle on both builds, so it is
  about 40px narrower.
- The drawer floor is unchanged up to about 166% UI scale. Above it the trailing
  end sets the floor: 8px wider at 175%, 32px at 200%.
- Opening and shutting the drawer no longer move the toggle. A Conversation-pane
  collapse shrinks it away in place, because the Canvas pane's hamburger
  arrives in the same corner. It no longer slides under the traffic lights on
  the way.
- The toggle draws a sidebar on desktop and a bulleted list on a phone.
- A filtered list no longer looks pressed. Filter is pressed only while its
  panel is open, and its glyph alone reports the filter.
- Filter's changes fade on `--duration-fast`, and are instant with motion
  reduced. Its pressed highlight moved from `--duration-normal` to that timing.
- The pane title's box is as wide as "Threads" or "Filters", whichever is
  wider, so the centred title no longer shifts when it swaps.
- The panel's box is `.thread-filter-cover`, and `.thread-filter-panel`
  inside it is always mounted. A closed panel is hidden, not gone, so a test
  asserts it is hidden rather than counting it.

## Alternatives considered

**Keep the travelling toggle and change only the glyph.** A distinct glyph makes
the toggle easy to recognise, but it is still not where the user looks. The glyph
change ships as well, on top of the fixed position.

**Pin the toggle and keep Filter on the left, after it.** That keeps Filter where
the mobile row has it. But the leading end would then carry the lights, the
toggle and Filter. The centred title pays that end twice, so the drawer floor
would grow by 80px on every client.

**Put the toggle at the drawer's far corner while open, as Safari does.** It is
close to the old position and has the same fault: the control moves between
states.

**Add a second close button inside the drawer.** Two controls would do one job,
and this header has already had to remove a second copy of the toggle once.

**Crossfade the panel with the list.** It is the usual view swap. Here the two
share one geometry, so the middle frames print rows over options.

**Mount the panel on open and fade it in from there.** It is the smaller change.
It is also the case WebKit starts a frame late, so the panel would trail the
header.

**Move Filter to the right on mobile too.** It would match the desktop row, at
the cost given under Consequences.
