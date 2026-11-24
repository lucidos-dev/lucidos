import type { ToastItem } from '../../store/types';

export function toastHasClose(
  t: Pick<ToastItem, 'dismissable' | 'persistent'>,
): boolean {
  return t.persistent === false || t.dismissable !== false;
}

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
