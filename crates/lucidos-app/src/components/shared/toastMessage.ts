import { clampText } from '../../utils/clampText';
import type { ToastType } from '../../store/types';

/** Longest ERROR message, in characters. An error is one sentence, and the
 *  longest the engine writes is around 180. A 390pt phone shows about 150 in
 *  the heading's six-line box, so a message at this budget is one short scroll
 *  rather than a page. No count can promise "always fits": the same string
 *  wraps to more lines on a 320pt screen than on a 430pt one. */
const ERROR_MAX_CHARS = 200;

/** Longest message of any other kind. A notification body may legitimately
 *  carry a list of lines. So this is a backstop against a pathological payload,
 *  not a style rule. An app reaching `showToast` through the toast bridge is
 *  bounded here too. */
const TOAST_MAX_CHARS = 2000;

/** What `showToast` stores for a toast's title or message, rather than what
 *  the caller passed.
 *
 *  An ERROR is flattened to one line as well as clamped. A failure is one
 *  sentence, so a response body put into one can never grow into a page. */
export function clampToastText(text: string, type: ToastType): string {
  if (type !== 'error') return clampText(text, TOAST_MAX_CHARS);
  return clampText(text.replace(/\s+/g, ' ').trim(), ERROR_MAX_CHARS);
}
