import { opensSoftwareKeyboard } from '../../utils/dom';
import { hasCoarsePointer, isMobile } from '../../utils/viewport';


export const isTouchLayout = (): boolean => isMobile() || hasCoarsePointer();

export function keyboardHolder(): HTMLElement | null {
  const el = document.activeElement;
  return isTouchLayout() && opensSoftwareKeyboard(el) ? (el as HTMLElement) : null;
}

export function returnKeyboard(holder: HTMLElement | null, panel: Element | null): boolean {
  if (!holder?.isConnected) return false;
  const active = document.activeElement;
  const unclaimed = !active || active === document.body || !!panel?.contains(active);
  if (!unclaimed) return false;
  holder.focus({ preventScroll: true });
  return true;
}
