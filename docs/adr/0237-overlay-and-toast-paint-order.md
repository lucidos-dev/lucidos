# 0237: Paint order follows what the reader is looking at: the overlay stack decides modal depth, and a modal outranks a toast that can wait

- **Status**: Accepted
- **Date**: 2026-09-21

## Context

Every `.modal-overlay` shared one `--z-modal`, and `.toast-container` sat above
all of them at `--z-toast`. Both were flat levels. So paint order among peers
fell out of DOM order, rather than out of anything meaning "what the reader is
attending to".

Two reports landed on the same day, and they are the same mistake twice.

A confirm raised BY a modal was drawn BEHIND it. `App.tsx` mounts
`<ConfirmDialog />` ahead of most modal slots, so the confirm lost on source
order. It was still the top of the `overlayStack` though, so it took the Escape
and swallowed the click, and the shell behind it was inert. Pressing the visible
button again only re-dismissed the invisible confirm.

The workspace read as frozen. The path in was the release-notice action, whose
"Replace your draft?" question only fires when a draft is in progress, so one
workspace looked broken and another fine.

Separately, the standing "Switch to new version" offer covered whatever modal
the reader opened under it. It said nothing they could not read a moment later.

## Decision

**A modal's level is its position in the `overlayStack`**, not a flat token.
`<Overlay>` writes `calc(var(--z-modal) + overlayStackDepth(...))` inline on its
backdrop container, so the overlay the dismiss contract already calls top is the
one the reader sees on top. `--z-modal` is the FLOOR of a band, capped by
`MAX_MODAL_STACK_DEPTH`.

**A modal outranks the toast layer, unless the stack holds a toast that cannot
wait.** A toast can wait when it persists and reports no problem. Anything else
keeps the whole stack above the band.

## Rationale

The stack already answered two questions: who takes Escape, and who answers a
pointer. It did not answer who is drawn on top, and the three have to agree.
A contract whose "top" is invisible is worse than no contract: it disables the
dismiss the user can see while arming one they cannot.

For the toast half, the test is what the reader loses by waiting. A standing
offer loses nothing, because it is still there afterwards. Two kinds do lose:

- **A timed toast** spends its timer under the scrim and is gone before the
  modal closes, so the reader is never told. This is why `showToast` records
  `persistent` rather than letting the urgency rule re-derive it. The rule
  mixes `autoDismissMs`, the key, the actions and the type, and a second copy
  of it would drift and lose a message in silence.
- **An error or a warning** does persist, so the reader gets it eventually. It
  still cannot wait. A failure raised by a button INSIDE the modal has to be
  seen while the modal is up. Otherwise the button reads as dead. See
  `.claude/rules/frontend.md` § No Hidden Errors.

The urgency answer is per STACK, not per toast: `.toast-container` carries the
z-index and is therefore one stacking context. One toast that cannot wait keeps
all of them on top. That is the safe direction. Covering a modal is a nuisance
and losing a message is a bug.

## Consequences

- `--z-modal` is a band floor. The blocker moved to `calc(var(--z-modal) + 50)`
  so it still covers every stacked modal, and `MAX_MODAL_STACK_DEPTH` is pinned
  under that gap by `ui-blocking-overlay-z-index.test.ts`.
- **An `overlayClass` may no longer name a z-index.** The inline band value
  beats a class rule, so one would be dead. It is dead silently, which is why
  `modal-overlay-z-index.test.ts` scans for it. Two existed. `.image-popup`
  resolved to the band floor and looked fine. `.camera-overlay` carried a raw
  `500` that its later-loading sheet used to win with, drawing the camera UNDER
  the header chrome.
- Every open backdrop overlay now subscribes to `overlayStack`, so opening or
  closing any overlay re-renders the others. Overlays are few and shallow and
  this happens at human-action frequency.
- The blocker case is carved out: `:root[data-ui-blocked]` stands the toast
  rule down, because there the "Refreshing…" toast is the only thing left to
  act on.

## Alternatives considered

**Move `<ConfirmDialog />` to the end of the `OverlayLayer` list.** One line,
and it fixes the reported case. It fixes nothing else: the next overlay added
after it re-opens the bug, and a confirm raised from `SearchEverywhere` (which
would still be later) stays broken. It also leaves the real defect standing,
which is that three answers to "which overlay is top" disagree.

**Render the overlay panels in stack order instead of assigning levels.** Same
end state, reached by reordering a static child list at runtime. It remounts
panels as the order changes, which costs every modal its DOM state for nothing.

**Sink the lower modals instead of raising the upper ones**, keeping the top one
at exactly `--z-modal`. Attractive because it needs no band and no blocker
change. Rejected because a modal's level would then change whenever anything
opened ABOVE it, an anchored dropdown sharing the stack included. The value
would move for reasons that have nothing to do with that modal.

**Lower the whole toast layer under any open modal.** The tempting one-rule fix,
and the one thing that must not happen. An error raised from inside a modal
would be invisible until the modal closed, so the button that raised it would
read as dead. That is the exact failure the modal-depth half of this ADR was
written to stop.

**Suppress standing toasts entirely while a modal is open, then re-show them.**
Needs a queue and a re-show trigger, and it can drop a toast whose owner
dismissed it meanwhile. Lowering it keeps one source of truth: the toast is
still mounted, still in the stack, just behind the thing the reader is reading.
