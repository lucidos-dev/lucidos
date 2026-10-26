# 0576: The composer row draws compact faces when the phone layout is on or the row is narrower than 36rem

- **Status**: Accepted
- **Date**: 2026-10-25
- **Amends**: ADR 0573 (the composer row's picker and action button keep one width per layout)

## Context

ADR 0573 gave the model picker and the action button one width per layout.
The layout was the viewport's. A desktop browser with a narrow thread pane
still drew the full picker and the 7.75rem pill, and they filled most of the
row. Only the fold reacted to the lack of room.
`docs/plans/2026-10-25-composer-row-compact-by-width.md` makes the fix
executable.

## Decision

**The composer row picks its face set from its own width.** The compact
faces are the intelligence badge alone and a round icon button. The row draws
them when the phone layout is on or the row is narrower than
`COMPOSER_COMPACT_MAX_REM` (36rem of content width). Otherwise it draws the desktop faces. A compact
split opens its menu on every tap, as the phone one did.

**One flag, two readers.** `useComposerCompact` measures the row and sets
`data-composer-compact` on it. The CSS face rules read the attribute.
`ActionSlot` and the fold's re-measure read the same value.

**Faces move, sizing stays.** Only the face rules left `@media
(--phone-layout)`. The 2rem picker height stays phone-only.

ADR 0573's "one width per layout" now reads "one width per face set". Its
other decisions stand.

## Rationale

**The pane is what runs out of room, not the screen.** A wide screen with a
narrow pane has the same squeeze a phone has. The row's own width is the
honest input.

**No feedback loop.** The row's width comes from the pane, never from its
members. Compact faces cannot widen the row back over the threshold.

**One definition.** A JS flag read by CSS keeps the face rules in one place.
A second copy of them under `@container` would drift from the phone rules.

## Consequences

- A narrow desktop pane loses the picker's name and the pill's word. The
  tooltip and the accessible name keep them.
- The fold forgets its remembered widths on a flip, so it never decides
  against the old face set.
- Rem keeps the threshold in step with the UI scale. A scale change alone
  leaves the row's px width alone, so the hook also re-measures on font
  metric changes (`useFontMetricsResize`).

## Alternatives considered

- **A `@container` query copying the phone face rules.** Rejected: two copies
  of the rules, and `ActionSlot` still needs the value in JS.
- **An intermediate face (a shorter pill with a word).** Rejected: two face
  sets keep the row simple, and the user asked only for the narrow case to
  shrink.
- **Folding harder at narrow widths.** Rejected: ADR 0573 chose to keep the
  picker and the action button standing and fold the rest.
