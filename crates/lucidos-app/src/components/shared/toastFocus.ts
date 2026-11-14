import type { ToastItem } from '../../store/types';

/** Timed toasts always offer an early exit. Persistent status toasts can
 *  withhold the close X with `dismissable: false`. Escape uses the same X. */
export function toastHasClose(
  t: Pick<ToastItem, 'dismissable' | 'persistent'>,
): boolean {
  return t.persistent === false || t.dismissable !== false;
}

/** Pure decision for the Tab trap over the toast that currently holds focus.
 *  Given the count of focusable controls in the toast (its action buttons, the
 *  close X, and any linkified URL), the active control's index among them,
 *  whether Shift is held, and whether an overlay currently owns the app, return:
 *
 *   - a NUMBER — the index to focus within the toast. Forward Tab steps to the
 *     next control and wraps off the last back to the first, so focus cycles
 *     through the toast's controls (incl. close) and never leaks into the pane
 *     mid-toast (the reason forward Tab is handled at every position, not just
 *     the boundary — an unhandled middle press would fall through to the
 *     focused-pane Tab trap and yank focus into the pane).
 *   - `'exit'` — Shift+Tab releases focus back to the focused pane, the single
 *     keyboard hatch out of the toast (Escape is the other, via the close X).
 *     After it, normal per-pane Tab resumes; the toast drops out of the cycle.
 *   - `null` — nothing to trap (`count === 0`): fall through to default Tab.
 *
 *  `overlayOpen` guards the exit. Focus can already be in a toast when an
 *  overlay opens, and the pane behind that overlay is not a valid Tab target:
 *  moving focus there would break the overlay's focus containment
 *  (`handleOverlayTab`). So while an overlay
 *  is open, Shift+Tab wraps backward within the toast instead of exiting.
 *
 *  It no longer follows that the toast is ON TOP of the overlay. A standing
 *  stack is drawn under an open modal (`components/shared/toastUrgency.ts`),
 *  and only an urgent one still paints over it. Containment is the right
 *  answer either way: the pane behind is no more reachable for being visible.
 *
 *  Pure (no DOM) so the trap logic is unit-tested; the caller resolves an index
 *  to a real element and 'exit' to the pane-focus move. */
export function toastTabTarget(
  count: number,
  activeIndex: number,
  shift: boolean,
  overlayOpen: boolean,
): number | 'exit' | null {
  if (count === 0) return null;
  if (shift) {
    if (!overlayOpen) return 'exit';
    return activeIndex <= 0 ? count - 1 : activeIndex - 1;
  }
  if (activeIndex < 0) return 0;
  return (activeIndex + 1) % count;
}
