# 0777: Window restore uses one logical point rect type and one shared clamp body for both moments

- **Status**: Accepted
- **Date**: 2026-11-22
- **Related**: [0173](0173-window-geometry-is-logical-points.md), [0193](0193-a-window-no-display-can-hold-is-corruption.md)

## Context

`window_restore.rs` judges window rects against the attached monitors and the
declared config floors. It runs at two moments. One is a rect the client is
about to write. The other is a rect a window already wears. The comments set
rules so these cannot drift apart.

This record restates the design comments of
`crates/lucidos-app/src/window_restore.rs`, written before the comment strip
removes them. Nobody checked it line by line against the code.

## Decision

- `Rect` holds window or monitor geometry in logical points, stored as `i64`.
- `window_session` persists the same `Rect` type, one per workspace.
- Each monitor carries two rects, because the clamp asks a display two
  different questions.
- Declared config values are rounded into whole points. No scale factor appears
  in the module.
- One reader returns the declared floors and the current desk. It returns
  `None` if any of it is unreadable, and every caller then leaves the geometry
  alone.
- The desk is read from the app, not from a window.
- Both clamps share one body. It reads the window's real geometry in points,
  asks the `decide` argument about it, and places the answer.
- A correction that must pick a monitor picks the one the rect overlaps most,
  while that overlap is at least a drag handle's worth.
- That pick answers the work area. `None` means a corner sliver or an unplugged
  display, and both start over on the primary.

## Rationale

- `i64` means an off-screen position far outside the desktop cannot underflow a
  subtraction or overflow an area product.
- One rect type means one set of units, so there is no second set to get wrong.
- The two monitor rects differ by the Dock.
- The module speaks the space the config is written in, so no scale factor is
  needed.
- One reader means judging a rect about to be written and judging one already
  worn cannot drift apart.
- The desk comes off the app because a restore must resolve a frame before its
  window exists.
- One shared body means the two moments cannot reach different arrangements for
  one window. They differ only in the question, which is `decide`.
- Deriving the threshold from the drag handle keeps one notion of enough window
  to work with.
- That threshold also separates a window hanging off an edge from the two cases
  with no place left.
- The work area is answered because every caller puts a window somewhere it has
  to live.

## Consequences

- An unreadable floor or desk never moves a window.
- A new caller supplies only a `decide` question, not its own geometry reader
  or placement code.
- A window overlapping every monitor by less than a drag handle is treated as
  unplaceable and restarts on the primary.
- A corner sliver and an unplugged display get the same treatment.

## Alternatives considered

- A second rect type for persistence, rejected because it would be a second set
  of units to get wrong.
- A percentage overlap threshold for choosing a monitor, rejected in favour of
  a threshold derived from the drag handle.
- Separate readers for a rect about to be written and one already worn,
  rejected because they could drift apart.
- Separate clamp bodies for the two moments, rejected because they could reach
  different arrangements for one window.
