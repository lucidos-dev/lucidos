# 0290: One surface anatomy for toasts, popovers, menus and dialogs; tone in the icon, one scrim, popovers drill in

- **Status**: Accepted (the explainer as a modal dialog is superseded by 0299, and the build popover by 0306)
- **Date**: 2026-09-26

## Context

Every floating surface had grown its own look. An inventory counted seven
radii, three border styles, four close affordances and four button families.
Toasts wore a thick tone-coloured frame, and `showConfirm` defaulted to a red
Delete, so Apply, Archive and Revert rendered as warnings. Five scrim opacities
dimmed the app, one of them behind a menu. The build-progress toast, opened
from the brand badge, covered the transcript with a changelog in a frame.

The user chose one option per situation from rendered mockups. The mockups and
the plan are in `docs/plans/2026-09-26-one-surface-system.md`.

## Decision

Every toast, popover, menu and dialog is one `.surface` box, with an optional
`SurfaceHead`, body and foot. The pieces live in `styles/global/surface.css` and
`components/shared/Surface.tsx`, on radius, fill, shadow and scrim tokens.
Tone lives in the head's icon, never in the frame. A confirm must state whether
it is destructive, and only a destructive one is red. Only a surface that blocks
takes the scrim. A popover never opens a second layer: detail drills in.

## Rationale

- **One anatomy makes consistency the default.** A new surface that uses the
  head, body and foot inherits the close X, the spacing and the buttons. It
  cannot invent a fifth close affordance by accident.
- **Tone in the icon keeps colour meaningful.** A coloured frame on every toast
  made colour the loudest thing on screen for a "Trigger saved". The icon says
  success or failure at the size the message deserves.
- **Red only for loss.** A required `variant` puts the choice in front of every
  caller, and `tsc` rejects one that skips it. The default used to decide for
  them, and it decided wrong for every safe action.
- **A scrim means "the app waits until you close this".** It marks a modal
  surface, one that traps focus and makes the app inert behind it. A read-only
  dialog such as an explainer or a detail modal is modal, so it dims too. A
  menu or a palette that dims the app claims an authority it does not have, and
  trains the eye to ignore the dim.
- **Drill-in keeps one layer.** A popover opened from a control is small and
  anchored. A modal stacked over it hides where the reader was and needs two
  closes. Replacing the content in place, with a back link, keeps them there.
  Escape steps back first, the way `ModelSelectionPicker`'s steps already did.

## Consequences

- `.action-btn-secondary` moves from the iframe-only stylesheet to the shared
  layer, since the host now uses it. Apps keep it unchanged.
- A dialog may still stack a confirm on itself, because a confirm needs an
  answer (ADR 0237's paint order).
- The image viewer and the camera keep a darker scrim of their own: they are
  lightboxes, not dialogs.
- The connection bar keeps its status dot as its tone mark, because it shares
  the Settings status-dot scale.
- The gateway workspace picker keeps its brand skin, with its popover colours
  named once in its own palette.
- The build status opened from the brand badge is an anchored popover. Work
  nobody tapped for (the embedding download, an Expose run from Settings) keeps
  the activity toast, which gives way while the popover is open.

## Alternatives considered

- **A tone-coloured frame everywhere.** Consistent, but it made the frame the
  loudest element on every surface, which was the complaint.
- **A tinted wash by tone**, as the banners use. Softer, but two tones side by
  side read as two different kinds of card rather than one card saying two
  things.
- **Bottom sheets on a phone** for confirms and detail modals. Thumb-friendly,
  but the phone and desktop would disagree on the same dialog, and a sheet over
  a sheet is the nesting problem again.
- **Stacking a modal over the popover**, as before. It worked, but it hid the
  anchor and cost two closes to get back to the thread.
- **Keeping `showConfirm`'s default and fixing the five callers.** It fixes
  today and leaves the next caller to inherit a red button by omission.
