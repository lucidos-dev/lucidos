/** Resize a textarea to fit its content, returning true if the height changed.
 *
 *  Four paths to avoid collapsing the textarea to height:0 on every keystroke:
 *  - Paste: text jumped by >1 char → collapse to 0 for fresh measurement
 *  - Fast: content fits and text didn't shrink → no-op (zero reflows)
 *  - Growth: content overflows → grow directly (one reflow)
 *  - Shrink: text deleted → collapse to 0 to measure true content height */

import { useEffect } from 'preact/hooks';

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

/** Cancels the in-flight height animation on a given textarea, if any. Keyed by
 *  element so a rapid second switch can tear the first animation's listener +
 *  timer down before starting its own — otherwise the stale `finish` would fire
 *  later and snap the box back to the previous switch's target height. */
const pendingHeightAnim = new WeakMap<HTMLTextAreaElement, () => void>();

/** Smoothly animate a textarea's height from a previous inline height to the one
 *  it currently rests at. The caller must have ALREADY applied the final height
 *  (e.g. via {@link resizeTextarea}) before calling — we read `el.style.height`
 *  as the target, invert to `fromHeight`, then transition back. Used for the
 *  draft→draft compose switch so the box eases to the new draft's size instead of
 *  snapping. Both endpoints are CSS `height` strings (not offsetHeight), so the
 *  interpolation is box-sizing-agnostic and lands exactly where resizeTextarea
 *  left it. No-op if the height didn't change. */
export function animateTextareaHeightFrom(el: HTMLTextAreaElement, fromHeight: string): void {
  // A switch mid-animation supersedes the previous one — tear it down first so
  // its listener/timer can't later clobber this run's target height.
  pendingHeightAnim.get(el)?.();

  const target = el.style.height;
  if (!target || !fromHeight || target === fromHeight) return;

  el.style.transition = 'none';
  el.style.height = fromHeight;
  void el.offsetHeight; // commit the start height before transitioning

  let timer: ReturnType<typeof setTimeout>;
  let raf1: number | undefined;
  let raf2: number | undefined;
  const cancel = () => {
    clearTimeout(timer);
    // Cancel the pending frames too — a same-frame re-switch calls cancel()
    // before raf1 fires, so without this the stale inner rAF would still add a
    // finish listener that never gets removed.
    if (raf1 !== undefined) cancelAnimationFrame(raf1);
    if (raf2 !== undefined) cancelAnimationFrame(raf2);
    el.removeEventListener('transitionend', finish);
    el.style.transition = '';
    pendingHeightAnim.delete(el);
  };
  const finish = (e?: TransitionEvent) => {
    if (e && (e.target !== el || e.propertyName !== 'height')) return;
    cancel();
    el.style.height = target;
  };
  pendingHeightAnim.set(el, cancel);

  raf1 = requestAnimationFrame(() => {
    raf2 = requestAnimationFrame(() => {
      el.addEventListener('transitionend', finish);
      el.style.transition = 'height 0.3s ease';
      el.style.height = target;
    });
  });
  // Safety net if transitionend never fires (e.g. tab hidden mid-switch).
  timer = setTimeout(finish, 400);
}

/** Re-measure when root font metrics change (UI scale, font family, font load).
 *  resizeTextarea() sets height in px — goes stale when root font-size changes
 *  (e.g. after loadPreferences applies the user's UI scale on page reload). */
export function useFontMetricsResize(onResize: () => void) {
  useEffect(() => {
    const root = document.documentElement;
    let lastScale = root.style.getPropertyValue('--user-ui-scale');
    let lastFont = root.style.getPropertyValue('--font-ui');
    let rafId: number | null = null;

    const observer = new MutationObserver(() => {
      const scale = root.style.getPropertyValue('--user-ui-scale');
      const font = root.style.getPropertyValue('--font-ui');
      if (scale !== lastScale || font !== lastFont) {
        lastScale = scale;
        lastFont = font;
        if (rafId === null) {
          rafId = requestAnimationFrame(() => { rafId = null; onResize(); });
        }
      }
    });
    observer.observe(root, { attributes: true, attributeFilter: ['style'] });

    document.fonts.addEventListener('loadingdone', onResize);

    return () => {
      if (rafId !== null) cancelAnimationFrame(rafId);
      observer.disconnect();
      document.fonts.removeEventListener('loadingdone', onResize);
    };
  }, []);
}
