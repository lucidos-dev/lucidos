/** Resize a textarea to fit its content, returning true if the height changed.
 *
 *  Four paths to avoid collapsing the textarea to height:0 on every keystroke:
 *  - Paste: text jumped by >1 char → collapse to 0 for fresh measurement
 *  - Fast: content fits and text didn't shrink → no-op (zero reflows)
 *  - Growth: content overflows → grow directly (one reflow)
 *  - Shrink: text deleted → collapse to 0 to measure true content height */

const cache = new WeakMap<HTMLTextAreaElement, { height: number; len: number }>();

/** Apply final height, manage overflow-y, cache, return whether height changed. */
function applyHeight(el: HTMLTextAreaElement, contentHeight: number, prevHeight: number, curLen: number): boolean {
  el.style.height = contentHeight + 'px';
  el.style.overflowY = 'hidden';
  const rendered = el.offsetHeight;
  cache.set(el, { height: rendered, len: curLen });
  if (rendered < contentHeight) {
    el.style.overflowY = 'auto';
  } else {
    el.scrollTop = 0;
  }
  return rendered !== prevHeight;
}

export function resizeTextarea(el: HTMLTextAreaElement): boolean {
  const cached = cache.get(el);
  const prevHeight = cached?.height ?? 0;
  const prevLen = cached?.len ?? -1;
  const curLen = el.value.length;

  // Paste / autocomplete: text jumped by more than one character.
  // Collapse to 0 so scrollHeight is measured fresh, not against stale height.
  if (prevLen >= 0 && curLen - prevLen > 1) {
    el.style.height = '0';
    return applyHeight(el, el.scrollHeight, prevHeight, curLen);
  }

  const scrollH = el.scrollHeight;
  const clientH = el.clientHeight;

  // Fast path: content fits and text didn't shrink — height is already correct.
  // Skip on first call (prevHeight === 0) so we always set an initial height.
  if (prevHeight > 0 && scrollH <= clientH && curLen >= prevLen) {
    if (cached) cached.len = curLen;
    return false;
  }

  // Growth: content overflows — grow without collapsing.
  if (scrollH > clientH) {
    return applyHeight(el, scrollH, prevHeight, curLen);
  }

  // Shrink: text deleted, content fits — collapse to measure true height.
  el.style.height = '0';
  return applyHeight(el, el.scrollHeight, prevHeight, curLen);
}
