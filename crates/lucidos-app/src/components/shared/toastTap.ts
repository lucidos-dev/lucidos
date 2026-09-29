import type { ToastItem } from '../../store/types';

/** What a tap on the toast card does.
 *
 *  - `'click'`: runs the toast's own `onClick`.
 *  - `'action'`: runs its one action, which then draws no button. A lone
 *    button repeats what the whole card can do.
 *  - `null`: nothing. The card keeps its buttons, and its X closes it.
 *
 *  Only an info or success toast gives its action to the card. A warning or an
 *  error is read, and a tap to read it must not act or close it. Only a NEUTRAL
 *  action becomes the tap: a `danger` or `confirm` one commits something. A
 *  spinning or progress toast narrates work in flight, so it is not passive. */
export type ToastTap = 'click' | 'action' | null;

export function toastTap(
  t: Pick<ToastItem, 'type' | 'onClick' | 'action' | 'secondaryAction'>,
): ToastTap {
  if (t.onClick) return 'click';
  if (t.type !== 'info' && t.type !== 'success') return null;
  if (t.secondaryAction) return null;
  if (t.action) return t.action.variant ? null : 'action';
  return null;
}
