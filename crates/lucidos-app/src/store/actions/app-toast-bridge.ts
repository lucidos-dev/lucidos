import { showToast, removeToast, defaultToastDurationMs } from '../store';
import type { ToastType } from '../types';

export interface AppToastPayload {
  message?: unknown;
  title?: unknown;
  type?: unknown;
  durationMs?: unknown;
  dismissable?: unknown;
  key?: unknown;
  spinning?: unknown;
}

const TOAST_TYPES = ['success', 'info', 'warning', 'error'] as const;

export function handleAppToastMessage(type: string, payload: AppToastPayload): boolean {
  if (type === 'lucidos:ui:toast') {
    if (typeof payload.message !== 'string' || payload.message.length === 0) return true;
    const toastType: ToastType = TOAST_TYPES.includes(payload.type as ToastType)
      ? payload.type as ToastType
      : 'info';
    const spinning = typeof payload.spinning === 'boolean' ? payload.spinning : undefined;
    const key = typeof payload.key === 'string' && payload.key.length > 0 ? payload.key : undefined;
    showToast(payload.message, toastType, {
      title: typeof payload.title === 'string' && payload.title.length > 0 ? payload.title : undefined,
      key,
      autoDismissMs: typeof payload.durationMs === 'number'
        ? payload.durationMs
        : defaultToastDurationMs(toastType, Boolean(spinning && key)),
      dismissable: typeof payload.dismissable === 'boolean' ? payload.dismissable : undefined,
      spinning,
    });
    return true;
  }

  if (type === 'lucidos:ui:dismissToast') {
    if (typeof payload.key === 'string' && payload.key.length > 0) removeToast(payload.key);
    return true;
  }

  return false;
}
