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
