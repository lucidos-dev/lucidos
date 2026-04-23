/** Returns true if the element is a text input (input, textarea, select, or contentEditable). */
export function isTextInput(el: EventTarget | Element | null): boolean {
  if (!(el instanceof HTMLElement)) return false;
  if (el.isContentEditable) return true;
  const tag = el.tagName;
  return tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT';
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
 *  only ever grow. */
export function autoResizeTextarea(el: HTMLTextAreaElement | null) {
  if (!el) return;
  el.style.height = 'auto';
  el.style.height = `${el.scrollHeight}px`;
}
