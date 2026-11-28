# 0776: Persistent shell bars share one wash style with no left stripe

- **Status**: Accepted
- **Date**: 2026-11-22
- **Related**: [0399](0399-content-title-grows-on-wide-panes.md), [0627](0627-content-title-minimal-reserve.md)

## Context

`shell.css` styles two parts of the app shell: the persistent bars under the
header, and the header rows that hold the title and the back and forward
chevrons.

The bars report a condition that stays true until something changes. The header
title used to sit in a flex middle, so it moved whenever the icons beside it
changed.

This record restates the design comments of
`crates/lucidos-app/src/styles/panels/shell.css`, written before the comment
strip removes them. Nobody checked it line by line against the code.

## Decision

**Persistent bars**

- A bar sits between the header and the panes for a lasting condition. It is
  not a toast, which reads as transient status in this app.
- Each bar has two mount points. On desktop it is a flow sibling in
  `.app-shell`. On mobile it is a child of the fixed `.app-header`. One set of
  rules serves both.
- Every bar wears a soft wash of its state colour over the surface and keeps
  the neutral border. The wash strength is one shared token.
- No bar has a coloured left stripe. `.claude/rules/frontend-css.md` bans it.
- The connection bar uses the same two states as the Lucidos menu notice. It is
  red for a settled disconnect and amber for a connect that has not landed.
- The connection bar has no motion. Its leading dot is the still one.
- The webhook ingress bar is red and has no dot. The refusal bar shares every
  one of its rules.
- On a phone, the webhook, refusal and connection-with-control bars wrap. The
  buttons drop to a second row at the trailing edge, and only the row gap is
  tightened.
- The backup reminder is left out of the phone wrap. The connection bar keeps
  its sentence beside its dot.

**Header title**

- The title cluster is absolutely centred on its own row, not between the
  clusters that flank it.
- The two chevrons are pinned to the ends of a fixed span, with the title
  between them.
- The content pane shares `--desktop-nav-span` with the thread pane, so the
  chevrons sit the same distance apart on both.
- A wide Canvas pane grows its own span past `--content-unfolded-reserve`, up
  to `--desktop-content-title-span-max`. The span still depends on pane width
  alone.
- The span floor is the cluster's natural width: two chevrons and the mark's
  tap target, touching.
- Mobile has no twin of the span token. Phone rows take everything outside the
  edge reserve, as `--header-nav-cluster-width` sets.
- In the packaged macOS build with the Canvas pane maximized, the title centres
  on the room left after the traffic lights.
- Non-overlap comes from the centred box's clamp, which clears
  `--content-side-reserve` at each end. Any reserve is derived from the
  controls, never hardcoded.

## Rationale

A toast reads as transient, so a lasting condition needs its own bar. One
shared wash token keeps the bars from drifting apart and gives the live style
remote one knob for the family.

The webhook fault is live, because a delivery sent now never reaches the
workspace. Amber would file it beside a nag, which is how eight hours of
silence went unnoticed. A wash that pulses under a sentence the user is reading
is noise on top of news.

The backup reminder's trailing X is a dismiss. Sent down to a row of actions,
it would read as a third action. Stacked bars use the row gap alone, because
the shared `--space-md` gap reads as a hole between a sentence and its button.

The eye reads the title against the pane under it. The trailing cluster is one
to three icon boxes wide, depending on the content view. A flex middle
therefore moved the title and both chevrons on every navigation. A fixed span
makes them hold still under the pointer.

## Consequences

- Three bars share one set of declarations, so a change lands once.
- The title and chevrons no longer move when the action icons change.
- A narrow pane squeezes the span, not the controls, down to the floor.
- On the packaged macOS build with Canvas maximized, the title is not centred
  on the pane. The threads title makes the opposite call for its own row,
  because its reserve is padding.
- A header popout that renders inline would need the token restore
  reintroduced. Today `.dropdown-menu` and `.nav-history-menu` portal to
  `<body>`, so none is needed.

## Alternatives considered

- A toast for lasting conditions. It reads as transient status.
- A coloured left stripe on any bar. It is banned outright.
- Amber for the webhook ingress bar. It would file a live fault beside a nag.
- A 3-zone flex row with the title as the `flex:1` middle. It centres on the
  gap, so the title slid on every navigation.
- A `translate(-50%)` title with a guessed `max-width: calc(100% - 12rem)`. The
  guess let the title slide under the icons.
- A picked 8rem span floor. Against a natural 6.6rem it left slack that pushed
  the back chevron onto the drawer toggle.
- A mobile twin of the desktop span token. A phone row has no width to spare.
