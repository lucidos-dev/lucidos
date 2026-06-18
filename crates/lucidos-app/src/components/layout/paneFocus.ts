import { type FocusedPane } from '../../store/store';
import { getVisiblePromptInput } from '../chat/promptFocus';
import { isMobile } from '../../utils/viewport';

/** CSS container for each desktop pane. The drawer is a sibling of the split
 *  layout; the thread/content panes are its two halves — all disjoint subtrees,
 *  so `Element.closest()` over the union resolves which pane a node lives in. */
const PANE_SELECTOR: Record<FocusedPane, string> = {
  drawer: '.thread-drawer',
  thread: '.pane-thread',
  content: '.pane-content',
};

/** Union selector for `closest()`. The three containers are disjoint subtrees
 *  (the drawer is a sibling of `.split-layout`), so a focused node resolves to
 *  exactly one — `closest` returns its nearest pane ancestor. */
const PANE_UNION = '.thread-drawer, .pane-thread, .pane-content';

/** Tabbable elements within a pane. Mirrors the set `ConfirmDialog` traps over,
 *  plus `iframe` (an app content pane). Visibility-filtered so a `display:none`
 *  control (collapsed section, hidden layout copy) never becomes a tab stop. */
const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), iframe, [tabindex]:not([tabindex="-1"])';

function visibleFocusables(container: HTMLElement): HTMLElement[] {
  return Array.from(container.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(
    (el) => el.getClientRects().length > 0,
  );
}

export function paneContainer(pane: FocusedPane): HTMLElement | null {
  return document.querySelector<HTMLElement>(PANE_SELECTOR[pane]);
}

/** Move real DOM focus to a pane's main control — the ⌘⇧ "focus pane" path
 *  (the pointer-down path stays signal-only so a click never has its focus
 *  stolen). thread → the message input (type immediately); content/drawer →
 *  the first tabbable element; falls back to the pane container itself. Deferred
 *  one frame because a pane that just expanded/opened isn't laid out yet, so its
 *  controls have no box and `.focus()` would no-op. Desktop-only (mobile
 *  navigates panes, it doesn't focus them). */
export function focusPaneMainControl(pane: FocusedPane): void {
  if (isMobile()) return;
  requestAnimationFrame(() => {
    const container = paneContainer(pane);
    if (!container) return;
    // The message input is the thread pane's main control, but it lives in the
    // pane subtree — guard with `contains` so we never grab a stale/mobile copy.
    let target: HTMLElement | null = pane === 'thread' ? getVisiblePromptInput() : null;
    if (!target || !container.contains(target)) {
      target = visibleFocusables(container)[0] ?? null;
    }
    (target ?? container).focus({ preventScroll: true });
  });
}

/** Pure boundary logic for the per-pane Tab trap: given the count of tabbable
 *  elements, the active element's index among them, and whether Shift is held,
 *  return the index to WRAP to, or `null` when no wrap is needed (the browser's
 *  default Tab keeps focus inside the contiguous pane subtree). Forward Tab off
 *  the last element wraps to the first; Shift+Tab off the first wraps to the
 *  last. An active element not in the set (index `-1`) never wraps. */
export function trapTargetIndex(
  count: number,
  activeIndex: number,
  shift: boolean,
): number | null {
  if (count === 0 || activeIndex < 0) return null;
  if (shift && activeIndex === 0) return count - 1;
  if (!shift && activeIndex === count - 1) return 0;
  return null;
}

/** Per-pane Tab trap. When focus is inside a pane — and no overlay is open
 *  (overlays own their own Tab via `overlayStack`) — keep Tab/Shift+Tab cycling
 *  within that pane. The browser handles the steps between the first and last
 *  tabbable (they're contiguous within a pane subtree); this only intercepts the
 *  boundary wrap. Returns `true` when it moved focus (caller preventDefaults).
 *  Desktop-only. */
export function handlePaneTab(e: KeyboardEvent): boolean {
  if (isMobile()) return false;
  // Overlays (modals, popovers) manage their own focus/Escape — don't fight them.
  if (document.documentElement.hasAttribute('data-overlay-open')) return false;
  const active = document.activeElement as HTMLElement | null;
  const container = active?.closest<HTMLElement>(PANE_UNION);
  if (!container) return false; // focus outside any pane → normal Tab
  const focusables = visibleFocusables(container);
  const target = trapTargetIndex(
    focusables.length,
    focusables.indexOf(active as HTMLElement),
    e.shiftKey,
  );
  if (target === null) return false;
  focusables[target].focus({ preventScroll: true });
  return true;
}
