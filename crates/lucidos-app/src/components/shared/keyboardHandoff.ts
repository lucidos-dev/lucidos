import { opensSoftwareKeyboard } from '../../utils/dom';
import { hasCoarsePointer, isMobile } from '../../utils/viewport';

/** The keyboard handoff, shared by every menu with a filter box.
 *
 *  A text field may hold the on-screen keyboard as a touch menu opens. The
 *  menu then moves focus to its own filter box, so typing filters the menu.
 *  Moving focus from one field to another keeps the keyboard up. A step with
 *  no text field hands focus back to the field in the same tap. Closing does
 *  too, so the keyboard stays up throughout. */

/** Whether menus follow the touch layout: a finger, or a phone-width window. */
export const isTouchLayout = (): boolean => isMobile() || hasCoarsePointer();

/** The text field holding the on-screen keyboard as a menu opens, or null.
 *  With no keyboard up, a menu raises none, since it would cover the list the
 *  user opened to tap. */
export function keyboardHolder(): HTMLElement | null {
  const el = document.activeElement;
  return isTouchLayout() && opensSoftwareKeyboard(el) ? (el as HTMLElement) : null;
}

/** Give focus back to `holder` as its menu closes, unless something outside
 *  `panel` has claimed it: a pick that moved focus on to a follow-up field
 *  keeps it. Returns true iff focus went back. */
export function returnKeyboard(holder: HTMLElement | null, panel: Element | null): boolean {
  if (!holder?.isConnected) return false;
  const active = document.activeElement;
  const unclaimed = !active || active === document.body || !!panel?.contains(active);
  if (!unclaimed) return false;
  holder.focus({ preventScroll: true });
  return true;
}
