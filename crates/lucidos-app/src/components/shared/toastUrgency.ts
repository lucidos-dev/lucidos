import type { ToastItem } from '../../store/types';

export type ToastUrgency = 'urgent' | 'standing';

function canWait(t: ToastItem): boolean {
  return t.persistent === true && t.type !== 'error' && t.type !== 'warning';
}

export function toastStackUrgency(items: readonly ToastItem[]): ToastUrgency {
  return items.every(canWait) ? 'standing' : 'urgent';
}
