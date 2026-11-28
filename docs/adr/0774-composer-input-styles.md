# 0774: The composer action row is one row that only folds

- **Status**: Accepted
- **Date**: 2026-11-22
- **Related**: [0221](0221-composer-rests-at-one-line.md), [0573](0573-composer-row-fixed-width-controls.md)

## Context

The chat styles in `input-messages.css` hold several layout rules that look
like gaps or accidents. Each one replaced or refused a different approach.

The composer's resting height is already settled in ADR 0221 and is not
repeated here.

This record restates the design comments of
`crates/lucidos-app/src/styles/chat/input-messages.css`, written before the
comment strip removes them. Nobody checked it line by line against the code.

## Decision

- The composer action row is one row. The fold is the only mechanism. Every
  member except the send button folds into the ⋯ menu.
- The frontend preview's direct actions in that menu are buttons, not
  `.control-item` rows.
- The transcript scroll region has `tabindex=0` but never shows a focus
  outline, in keyboard or pointer focus.
- The initiator header's right-aligned cluster is deliberately identical to
  `.response-meta` in `chat/response.css`. It stays a separate copy.
- `styles/__tests__/turn-meta-stacking.test.ts` holds the two copies together.
- The turn controls sit immediately right of the status chip on both headers.
  They use one gap for all three icons, the collapse control included.
- A user message and a change turn carry no fold, so no run of controls ever
  leads a row.

## Rationale

- The old `flex-wrap: wrap-reverse` net and the `is-stacked` column variant
  both drew a second line above a phone's keyboard. They also disagreed with
  the fold about which should give way. With only the fold, there is always a
  count the row can hold, so nothing is left to wrap.
- The preview actions act on a supervised process here and now. Every other row
  in the menu selects a command to send to the agent session.
- A whole-pane ring around the conversation while arrow-scrolling reads as
  chrome, not affordance. The per-turn marker `.nav-focus-stuck` already shows
  the focused turn by washing it.
- The two headers sit one above the other in a turn, so they have to degrade
  the same way. Each header's rules live with its own component, so they are
  not one shared selector.
- The collapse control used to carry an extra `0.3125rem` gap to mark a group
  break. Its label and turning glyph already carry that scope. Two different
  gaps inside a run of three icons read as two things and a stray.

## Consequences

- A second row in the composer cluster cannot be drawn, because nothing wraps.
- The transcript pane has no visible focus ring. Focus is shown only through
  the per-turn wash.
- A change to the meta cluster must be made in both CSS files. The test fails
  if the two copies drift apart.
- The collapse control keeps its class, since the pressed-chip rule still
  singles it out by that class.

## Alternatives considered

- `flex-wrap: wrap-reverse` as a safety net on the composer cluster.
- An `is-stacked` column variant that lifted the Diff button onto a sub-row.
- Preview actions drawn as `.control-item` rows.
- A focus outline on the whole transcript pane.
- One shared selector for the two header meta clusters.
- An extra gap before the collapse control to mark a group break.
