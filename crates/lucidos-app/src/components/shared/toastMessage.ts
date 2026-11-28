import { clampText } from '../../utils/clampText';
import type { ToastType } from '../../store/types';

export const ERROR_MAX_CHARS = 200;

export const TOAST_MAX_CHARS = 2000;

export function clampToastText(text: string, type: ToastType): string {
  if (type !== 'error') return clampText(text, TOAST_MAX_CHARS);
  return clampText(text.replace(/\s+/g, ' ').trim(), ERROR_MAX_CHARS);
}
