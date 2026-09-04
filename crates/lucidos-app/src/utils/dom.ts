/** Focus an element only if it isn't already the active element, avoiding flicker.
 *  Uses preventScroll to avoid iOS Safari auto-scrolling overflow:hidden
 *  containers (e.g. the mobile swipe container) when the target is offscreen. */
export function focusIfNeeded(el: HTMLElement | null | undefined): void {
  if (el && document.activeElement !== el) {
    el.focus({ preventScroll: true });
  }
}

/** Returns true if the element is a text input (input, textarea, select, or contentEditable). */
export function isTextInput(el: EventTarget | Element | null): boolean {
  if (!(el instanceof HTMLElement)) return false;
  if (el.isContentEditable) return true;
  const tag = el.tagName;
  return tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT';
}

/** True if the element, or any ancestor, is an interactive control that owns its
 *  own tap/drag (button, link, form field, [role="button"], contentEditable).
 *  Used to exempt edge controls from the iOS navigation-swipe guard so their
 *  taps still fire — preventDefault on a touchstart swallows the emulated click. */
export function isInteractiveTarget(el: EventTarget | null): boolean {
  if (!(el instanceof Element)) return false;
  return !!el.closest('button, a, input, textarea, select, label, [role="button"], [contenteditable]');
}

/** True if the element is, or sits within, the focusable conversation transcript
 *  scroll region (`.thread-content`). Used so a Space keypress while that region
 *  has focus pages it down instead of being captured by type-to-focus-prompt. */
export function isThreadTranscript(el: EventTarget | null): boolean {
  return el instanceof HTMLElement && el.closest('.thread-content') !== null;
}

const KEYBOARD_INPUT_TYPES = new Set([
  'text', 'search', 'email', 'password', 'tel', 'url', 'number',
]);

/** True only for elements that bring up the on-screen keyboard when focused.
 *  Excludes range, checkbox, button, color, file, date, select, etc. — these
 *  focus without opening a keyboard, so triggering keyboard-related layout
 *  (mobile header hide-on-focus) for them is wrong. Use `isTextInput` for
 *  "is the user actively editing"; use this for "did focus open the OS keyboard". */
export function opensSoftwareKeyboard(el: EventTarget | Element | null): boolean {
  if (!(el instanceof HTMLElement)) return false;
  if (el.isContentEditable) return true;
  if (el.tagName === 'TEXTAREA') return true;
  if (el.tagName !== 'INPUT') return false;
  // HTMLInputElement.type is a reflecting getter — normalizes to 'text' for
  // unset/unknown values, never returns ''.
  return KEYBOARD_INPUT_TYPES.has((el as HTMLInputElement).type);
}

/** Pixels per CSS rem at the document root. Mobile uses 112.5% (18px) base;
 *  the 16 fallback covers SSR/JSDOM where getComputedStyle returns ''. */
export function getRemPx(): number {
  return parseFloat(getComputedStyle(document.documentElement).fontSize) || 16;
}

/** The viewport clamps live in `@lucidos/geometry`, because the shared tooltip
 *  runs in an app iframe and needs them too. Re-exported here so the host's own
 *  callers keep one import site, and there is still one definition. */
export { clampWithin } from '@lucidos/geometry';

/** Resize a textarea to fit its content. Setting height to 'auto' first lets
 *  scrollHeight shrink when text is removed; without it the textarea would
 *  only ever grow.
 *
 *  scrollHeight is content + padding (no border). With the project's global
 *  box-sizing: border-box, CSS height includes the border, so the content area
 *  shrinks by border-height and clips the bottom of descenders (g, p, y, j, q).
 *  Add the border back so the content area exactly fits scrollHeight. */
export function autoResizeTextarea(el: HTMLTextAreaElement | null) {
  if (!el) return;
  el.style.height = 'auto';
  const cs = getComputedStyle(el);
  // `|| 0` for the same reason `getRemPx` carries `|| 16`: getComputedStyle
  // answers '' under SSR/JSDOM, and a NaN height is a silently dropped
  // declaration that leaves the textarea stuck at `auto`.
  const borderY = (parseFloat(cs.borderTopWidth) || 0) + (parseFloat(cs.borderBottomWidth) || 0);
  el.style.height = `${el.scrollHeight + borderY}px`;
}
