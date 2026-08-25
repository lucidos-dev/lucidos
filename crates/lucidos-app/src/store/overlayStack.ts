/** Central LIFO registry of dismissable overlays (modals, the search palette,
 *  dropdowns, pseudo-fullscreen). Replaces the per-instance `document` Escape
 *  listeners that each overlay used to register independently — those raced
 *  each other and any future global key handler. Now ONE capture-phase Escape
 *  dispatcher (see `useKeyboardShortcuts`) pops the top entry, and
 *  `resolveGlobalActions` surfaces a top-priority `dismiss_overlay` action when
 *  the stack is non-empty.
 *
 *  An overlay registers its dismiss handler on mount and removes it on unmount.
 *  Calling `dismiss()` closes the overlay (it flips whatever signal renders it),
 *  whose unmount cleanup then removes the entry — so the stack stays in sync
 *  with what's actually on screen without the registrant having to remove
 *  itself eagerly. */

import { signal } from '@preact/signals';

export interface OverlayEntry {
  /** Stable id for this overlay instance (used to remove on unmount). */
  id: string;
  /** Close this overlay. Idempotent — safe to call when already closing. */
  dismiss: () => void;
  /** True for an `<Overlay>`, which owns a panel on screen. False for an
   *  **Escape-only registrant**, which owns no pixels: a step inside somebody
   *  else's panel (`ModelSelectionPicker`), pseudo-fullscreen, the thread
   *  filter. `topPanelOverlay` below is the only reader.
   *
   *  REQUIRED, not optional, because the whole pointer half of the dismiss
   *  contract hangs off it. Optional, a push that forgot it still type-checked
   *  and passed every unit test. `topPanelOverlay` then answered null for every
   *  stack state, switching outside-click dismiss off app-wide in silence. */
  hasPanel: boolean;
}

export const overlayStack = signal<readonly OverlayEntry[]>([]);

export function pushOverlay(entry: OverlayEntry): void {
  // Replace any stale entry with the same id (re-mount without a clean unmount)
  // so the stack never holds two handlers for one logical overlay.
  const without = overlayStack.value.filter((e) => e.id !== entry.id);
  overlayStack.value = [...without, entry];
}

export function removeOverlay(id: string): void {
  const next = overlayStack.value.filter((e) => e.id !== id);
  if (next.length !== overlayStack.value.length) overlayStack.value = next;
}

/** The most-recently-pushed (top) overlay, or null when the stack is empty. */
export function topOverlay(): OverlayEntry | null {
  const s = overlayStack.value;
  return s.length > 0 ? s[s.length - 1] : null;
}

/** The most-recently-pushed entry that OWNS A PANEL, or null when none does.
 *
 *  **Escape asks `topOverlay`; a POINTER asks this.** An Escape-only registrant
 *  is on the stack to answer the key, and sitting above the panel is what makes
 *  a step inside it answer first. It draws nothing though. So no pointer can
 *  land on one, and letting it shadow the panel underneath would switch that
 *  panel's outside-click dismiss off.
 *
 *  That is not hypothetical. `ModelSelectionPicker` registers a step above its
 *  host menu on purpose. Every model menu in the app would stop answering an
 *  outside click while a tier row was open. */
export function topPanelOverlay(): OverlayEntry | null {
  const s = overlayStack.value;
  for (let i = s.length - 1; i >= 0; i--) {
    if (s[i].hasPanel) return s[i];
  }
  return null;
}

/** Dismiss the top overlay. Returns true iff one was present and dismissed. */
export function dismissTopOverlay(): boolean {
  const top = topOverlay();
  if (!top) return false;
  top.dismiss();
  return true;
}

export function _resetOverlayStackForTesting(): void {
  overlayStack.value = [];
}
