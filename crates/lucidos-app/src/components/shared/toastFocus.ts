import type { ToastItem } from '../../store/types';

/** Which control of an action-bearing toast should receive focus when it
 *  appears, so the user can act with Enter instead of reaching for the mouse:
 *
 *   - `'primary'` / `'secondary'` — the action button, preferring the primary,
 *     but NEVER a destructive (`variant: 'danger'`) one: a reflexive Enter on a
 *     freshly-appeared toast must not fire something like "Cancel Apply-All".
 *   - `'close'`  — fall back to the dismiss (X), which only closes the toast.
 *   - `null`     — don't move focus at all. Returned when the toast carries no
 *     actions (a plain info/success toast shouldn't steal focus) or when the
 *     only thing left to focus is a destructive action with no safe dismiss
 *     (the non-dismissable Apply-All progress toast): better to leave focus put
 *     than to pre-arm Enter on a footgun. The button stays reachable via Tab.
 *
 *  Pure (no DOM) so the selection is unit-tested; the caller resolves the
 *  returned slot to a real element and focuses it. */
export function toastAutofocusTarget(
  t: Pick<ToastItem, 'action' | 'secondaryAction' | 'dismissable'>,
): 'primary' | 'secondary' | 'close' | null {
  if (!t.action && !t.secondaryAction) return null;
  if (t.action && t.action.variant !== 'danger') return 'primary';
  if (t.secondaryAction && t.secondaryAction.variant !== 'danger') return 'secondary';
  if (t.dismissable !== false) return 'close';
  return null;
}
