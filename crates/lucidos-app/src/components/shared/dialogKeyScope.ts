/** Which of the centered dialogs a `document`-level keystroke belongs to.
 *
 *  `ConfirmDialog` and `PromptDialog` render from INDEPENDENT signals
 *  (`confirmState` / `promptState`), and `showConfirm` / `showPrompt` each
 *  replace only a prior dialog of their OWN kind, so a confirm and a prompt can
 *  be on screen at the same time (both are mounted side by side in `App.tsx`).
 *  Each installs its own bubble-phase `document` keydown listener, and
 *  `preventDefault()` does not stop a sibling listener on the same node, so both
 *  ran on every Enter: a value typed into the prompt's single-line input
 *  submitted the prompt AND answered the confirm behind it with "yes",
 *  committing a destructive action (delete an app, discard a change, archive a
 *  thread) the reader never confirmed. Only `stopImmediatePropagation` would
 *  stop the sibling, and that merely makes the outcome depend on listener
 *  registration order.
 *
 *  So each dialog asks whether the keystroke is its own, in two steps.
 *
 *  One that originated inside SOME overlay panel belongs to that panel and to no
 *  other, which covers the sibling dialog and every other open overlay (a
 *  `Dropdown`'s free-text filter, the search palette).
 *
 *  One that originated outside every panel belongs to the TOP overlay only.
 *  Clicking a dialog's own message text leaves focus on `document.body`, and a
 *  bare Enter there must still answer the dialog, so "outside" cannot simply
 *  disown. But it cannot simply own either: with a prompt stacked over a confirm
 *  and focus on body, owning would fire BOTH listeners again, which is the whole
 *  bug. Asking "am I the top overlay?" answers exactly one, the one the reader is
 *  looking at.
 *
 *  `[data-overlay-panel]` is stamped by `<Overlay>` on the exact node each dialog
 *  holds in its `panelRef`, and its VALUE is that overlay's `overlayStack` id, so
 *  the panel can be matched back to its stack entry. Both lookups are duck-typed:
 *  a target without `closest` reads as "outside every panel", and a panel that
 *  cannot report an id (a test fake, a panel not built by `<Overlay>`) keeps
 *  answering, since refusing on an unknown would silently break Enter. */
import { topOverlay } from '../../store/overlayStack';

export function dialogOwnsKey(target: EventTarget | null, panel: HTMLElement | null): boolean {
  const el = target as { closest?: (selector: string) => Element | null } | null;
  const owner = typeof el?.closest === 'function' ? el.closest('[data-overlay-panel]') : null;
  if (owner !== null) return owner === panel;
  return panelIsTopOverlay(panel);
}

/** Whether `panel` is the top entry of the overlay stack. True when it cannot be
 *  determined, so an unknown never costs the reader a working Enter key. */
function panelIsTopOverlay(panel: HTMLElement | null): boolean {
  const el = panel as { getAttribute?: (name: string) => string | null } | null;
  const ownId = typeof el?.getAttribute === 'function' ? el.getAttribute('data-overlay-panel') : null;
  if (!ownId) return true;
  const top = topOverlay();
  return top === null || top.id === ownId;
}
