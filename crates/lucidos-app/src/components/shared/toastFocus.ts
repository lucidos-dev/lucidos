import type { ToastItem } from '../../store/types';

/** Which control of an action-bearing toast should receive focus when it
 *  appears, so the user can act with Enter instead of reaching for the mouse:
 *
 *   - `'primary'` / `'secondary'` — the action button, preferring the primary,
 *     but NEVER a destructive (`variant: 'danger'`) one: a reflexive Enter on a
 *     freshly-appeared toast must not fire something like "Cancel Apply-All".
 *   - `'close'`  — fall back to the dismiss (X), which only closes the toast.
 *   - `null`     — don't move focus at all. Returned when the toast carries no
 *     actions (a plain info/success toast shouldn't steal focus), when it opts
 *     out via `noAutofocus` (an UNSOLICITED toast — a notification toast — must
 *     not yank focus mid-typing / pre-arm a reflexive Enter on its default
 *     action, e.g. a notification's [Open]), or when the
 *     only thing left to focus is a destructive action with no safe dismiss
 *     (the non-dismissable Apply-All progress toast): better to leave focus put
 *     than to pre-arm Enter on a footgun. The button stays reachable via Tab.
 *
 *  Pure (no DOM) so the selection is unit-tested; the caller resolves the
 *  returned slot to a real element and focuses it. */
export function toastAutofocusTarget(
  t: Pick<ToastItem, 'action' | 'secondaryAction' | 'dismissable' | 'noAutofocus'>,
): 'primary' | 'secondary' | 'close' | null {
  if (t.noAutofocus) return null;
  if (!t.action && !t.secondaryAction) return null;
  if (t.action && t.action.variant !== 'danger') return 'primary';
  if (t.secondaryAction && t.secondaryAction.variant !== 'danger') return 'secondary';
  if (t.dismissable !== false) return 'close';
  return null;
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
 *  `overlayOpen` guards the exit: a toast stays interactive ABOVE an open
 *  modal/popover, so focus can land on it while an overlay owns the app — but
 *  the pane is then BEHIND the overlay and not a valid Tab target (moving focus
 *  there would break the overlay's focus containment, the way `handlePaneTab`
 *  and the toast auto-focus already yield to `data-overlay-open`). So while an
 *  overlay is open, Shift+Tab wraps backward within the toast instead of
 *  exiting, keeping focus contained above the overlay.
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
