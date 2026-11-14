import type { ToastItem } from '../../store/types';

/** Whether the toast stack may be drawn under an open modal.
 *
 *  `standing` only when every toast in it will still be there afterwards and
 *  none reports a problem. `urgent` otherwise. */
export type ToastUrgency = 'urgent' | 'standing';

/** Whether this toast may wait behind a modal.
 *
 *  Two things disqualify it, and they fail differently.
 *
 *  A TIMED toast is removed on its own schedule. Lowered, it spends that timer
 *  under the scrim and is gone before the modal closes, so the reader is never
 *  told what it said. `showToast` records `persistent`, which is the one place
 *  that knows the rule.
 *
 *  An error or a warning does persist, so the reader would get it eventually.
 *  It still cannot wait. A failure raised by a button INSIDE the modal has to
 *  be seen while the modal is up. Otherwise the button reads as dead. See
 *  `.claude/rules/frontend.md` § No Hidden Errors.
 */
function canWait(t: ToastItem): boolean {
  return t.persistent === true && t.type !== 'error' && t.type !== 'warning';
}

/**
 * What the stack is allowed to cover, for the CSS rule keyed on it.
 *
 * A modal is what the reader is looking at, so it outranks the toast layer. A
 * standing offer does not compete with it: "Switch to new version" waits until
 * the modal closes and says the same thing then.
 *
 * One answer for the whole stack, because `.toast-container` carries the
 * z-index and is therefore one stacking context. A per-toast level cannot
 * escape it. So ONE toast that cannot wait keeps the whole stack on top. That
 * is the safe direction: covering a modal is a nuisance, and losing a message
 * is a bug.
 */
export function toastStackUrgency(items: readonly ToastItem[]): ToastUrgency {
  return items.every(canWait) ? 'standing' : 'urgent';
}
