import { useLayoutEffect, useRef, useState } from 'preact/hooks';

/** Whether chips needing `need` pixels overflow a row with `room` pixels. Half
 *  a pixel of slack absorbs fractional layout widths. */
export function chipsOverflow(need: number, room: number): boolean {
  return need > room + 0.5;
}

/** The width the shelf's chips take, margins included. */
function shelfNeed(shelf: HTMLElement): number {
  const style = getComputedStyle(shelf);
  return shelf.scrollWidth + parseFloat(style.marginLeft) + parseFloat(style.marginRight);
}

/** The widest line the title row can give the shelf: the shelf wraps under
 *  the title before it shrinks. */
function rowRoom(row: HTMLElement): number {
  const style = getComputedStyle(row);
  return row.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight);
}

/** Whether the shelf's chips drop their labels and show their icons only
 *  (ADR 0414, invariant I16). The labelled width is measured while the labels
 *  show, and kept for this set of chips. So hiding the labels never makes the
 *  shelf look roomy enough to bring them back. */
export function useShelfTight(shelf: HTMLElement | null, chipsKey: string): boolean {
  const [tight, setTight] = useState(false);
  const labelledNeed = useRef<number | null>(null);
  const measuredKey = useRef(chipsKey);

  useLayoutEffect(() => {
    const row = shelf?.parentElement;
    if (!shelf || !row) return;
    if (measuredKey.current !== chipsKey) {
      measuredKey.current = chipsKey;
      labelledNeed.current = null;
      if (tight) {
        setTight(false);
        return;
      }
    }
    const measure = () => {
      if (!tight) labelledNeed.current = shelfNeed(shelf);
      if (labelledNeed.current !== null) setTight(chipsOverflow(labelledNeed.current, rowRoom(row)));
    };
    measure();
    // A label measured in the fallback font is narrower than the real one.
    document.fonts?.addEventListener('loadingdone', measure);
    const observer = typeof ResizeObserver === 'function' ? new ResizeObserver(measure) : null;
    observer?.observe(row);
    return () => {
      document.fonts?.removeEventListener('loadingdone', measure);
      observer?.disconnect();
    };
  }, [shelf, chipsKey, tight]);

  return tight;
}
