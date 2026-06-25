import { focusedPane, type FocusedPane } from '../../store/store';
import { getVisiblePromptInput } from '../chat/promptFocus';
import { isMobile } from '../../utils/viewport';

/** CSS container for each desktop pane. The drawer is a sibling of the split
 *  layout; the thread/content panes are its two halves — all disjoint subtrees,
 *  each resolved from the `focusedPane` signal via `paneContainer`. */
const PANE_SELECTOR: Record<FocusedPane, string> = {
  drawer: '.thread-drawer',
  thread: '.pane-thread',
  content: '.pane-content',
};

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

/** Pure target logic for the per-pane Tab trap, anchored on the FOCUSED pane.
 *  Given the count of tabbable elements in the focused pane, the active element's
 *  index among them, and whether Shift is held, return the index to focus, or
 *  `null` to fall through to the browser's default Tab.
 *
 *  - `activeIndex < 0` — DOM focus is OUTSIDE the focused pane (on `<body>` after
 *    a signal-only pane click, on the `tabindex=-1` pane container, or in another
 *    pane). Pull focus IN: forward Tab → first element, Shift+Tab → last.
 *  - inside the focused pane — wrap at the boundaries via `trapTargetIndex`;
 *    `null` in-between lets the browser step through the contiguous subtree.
 *  - `count === 0` — nothing to focus → fall through.
 *
 *  This is what makes Tab respect the focused panel even when DOM focus never
 *  entered it (a click sets `focusedPane` signal-only — see `focusPane`). */
export function paneTabTarget(
  count: number,
  activeIndex: number,
  shift: boolean,
): number | null {
  if (count === 0) return null;
  if (activeIndex < 0) return shift ? count - 1 : 0;
  return trapTargetIndex(count, activeIndex, shift);
}

/** Per-pane Tab trap. While no overlay is open (overlays own their own Tab via
 *  `overlayStack`), Tab/Shift+Tab cycle within the FOCUSED pane — and move INTO
 *  it when DOM focus is currently elsewhere. Anchored on `focusedPane` (the
 *  user's intent), not `document.activeElement.closest()`: a pane click sets the
 *  focused pane signal-only and never moves DOM focus, so keying off the active
 *  element let Tab escape to document order (or cycle the wrong pane) after a
 *  click. Switch panes with the ⌘⇧ pane shortcuts or a click. Returns `true`
 *  when it moved focus (caller preventDefaults). Desktop-only. */
export function handlePaneTab(e: KeyboardEvent): boolean {
  if (isMobile()) return false;
  // Overlays (modals, popovers) manage their own focus/Escape — don't fight them.
  if (document.documentElement.hasAttribute('data-overlay-open')) return false;
  const container = paneContainer(focusedPane.value);
  if (!container) return false; // focused pane not in the DOM → normal Tab
  const focusables = visibleFocusables(container);
  const active = document.activeElement as HTMLElement | null;
  const target = paneTabTarget(
    focusables.length,
    active ? focusables.indexOf(active) : -1,
    e.shiftKey,
  );
  if (target === null) return false;
  focusables[target].focus({ preventScroll: true });
  return true;
}
