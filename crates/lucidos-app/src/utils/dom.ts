/** Returns true if the element is a text input (input, textarea, select, or contentEditable). */
export function isTextInput(el: EventTarget | Element | null): boolean {
  if (!(el instanceof HTMLElement)) return false;
  if (el.isContentEditable) return true;
  const tag = el.tagName;
  return tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT';
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

/** Clamp a horizontal `left` coordinate so a `width`-wide element fits inside
 *  the `[min, max]` range, leaving `margin` px of breathing room on either edge. */
export function clampLeftWithin(left: number, width: number, min: number, max: number, margin = 8): number {
  return Math.max(min + margin, Math.min(left, max - width - margin));
}

/** Clamp a horizontal `left` coordinate so a `width`-wide element stays inside the viewport,
 *  leaving `margin` px of breathing room on either edge. */
export function clampToViewportX(left: number, width: number, margin = 8): number {
  return clampLeftWithin(left, width, 0, window.innerWidth, margin);
}

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
  const borderY = parseFloat(cs.borderTopWidth) + parseFloat(cs.borderBottomWidth);
  el.style.height = `${el.scrollHeight + borderY}px`;
}
