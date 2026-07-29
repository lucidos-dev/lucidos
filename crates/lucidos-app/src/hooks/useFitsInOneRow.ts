import { useState, useLayoutEffect } from 'preact/hooks';
import type { RefObject } from 'preact';
import { getRemPx } from '../utils/dom';

/** Pure width math, exported so the hook's decision can be unit-tested
 *  without ResizeObserver/MutationObserver/jsdom quirks. Sums each item's
 *  measured width plus a uniform `gapPx` between adjacent items, then
 *  compares to `containerWidth` (with a 0.5px sub-pixel rounding fudge).
 *  Empty list = trivially fits. */
export function computeFitsInOneRow(
  itemWidths: readonly number[],
  containerWidth: number,
  gapPx: number,
): boolean {
  if (itemWidths.length === 0) return true;
  let total = 0;
  for (const w of itemWidths) total += w;
  total += (itemWidths.length - 1) * gapPx;
  return total <= containerWidth + 0.5;
}

/**
 * Whether every `[data-row-item]` descendant of `containerRef` would fit in a
 * single row at `containerRef.clientWidth`. Sums each item's measured width
 * (so the result is stable across stacked / unstacked re-renders — items keep
 * the same intrinsic width regardless of which subrow they end up in) plus a
 * uniform `gapRem` between adjacent items, then compares to clientWidth.
 *
 * `gapRem` is multiplied by the document root font size at measure time so
 * user font scaling and viewport zoom feed in directly — no hard-coded px
 * thresholds and no viewport-width heuristics.
 *
 * Re-measures on container resize (ResizeObserver) and on subtree mutation
 * (MutationObserver); button labels can change at runtime ("Apply" →
 * "Apply & Restart") and any width change must trigger a recomputation.
 */
export function useFitsInOneRow(
  containerRef: RefObject<HTMLElement>,
  gapRem = 0.5,
): boolean {
  const [fits, setFits] = useState(true);
  useLayoutEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    const measure = () => {
      const items = container.querySelectorAll<HTMLElement>('[data-row-item]');
      const widths: number[] = [];
      items.forEach((item) => {
        widths.push(item.getBoundingClientRect().width);
      });
      setFits(computeFitsInOneRow(widths, container.clientWidth, gapRem * getRemPx()));
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(container);
    const mo = new MutationObserver(measure);
    mo.observe(container, {
      childList: true,
      subtree: true,
      characterData: true,
    });
    return () => {
      ro.disconnect();
      mo.disconnect();
    };
  }, [containerRef, gapRem]);
  return fits;
}
