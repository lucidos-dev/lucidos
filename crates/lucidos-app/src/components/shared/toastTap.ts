import type { ToastItem } from '../../store/types';

export type ToastTap = 'click' | 'action' | null;

export function toastTap(
  t: Pick<ToastItem, 'type' | 'onClick' | 'action' | 'secondaryAction'>,
): ToastTap {
  if (t.onClick) return 'click';
  if (t.type !== 'info' && t.type !== 'success') return null;
  if (t.secondaryAction) return null;
  if (t.action) return t.action.variant || t.action.deliberate ? null : 'action';
  return null;
}
