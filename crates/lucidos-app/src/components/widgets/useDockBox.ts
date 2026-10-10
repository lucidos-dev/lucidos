import { useLayoutEffect, useState } from 'preact/hooks';
import { focusedThreadId } from '../../store/store';
import type { Box } from '../../store/widgetWindows';
import { getRemPx } from '../../utils/dom';

/** The title rows a docked widget window hangs under, as the shelf drop does. */
const TITLE_ROWS = '.thread-view-header, .mobile-thread-title-row';

/** The mounted layout's title row. Only one layout's pane tree is mounted, so
 *  the first with a size is it, wherever a phone swipe has moved it. */
function titleRow(): HTMLElement | null {
  return [...document.querySelectorAll<HTMLElement>(TITLE_ROWS)].find((el) => el.getBoundingClientRect().width > 0) ?? null;
}

/** With no title row, such as the empty compose view: under the app header,
 *  inside the page gutter. */
function underHeader(): Box {
  const gap = 0.5 * getRemPx();
  const top = document.querySelector('.app-header')?.getBoundingClientRect().bottom ?? 0;
  return { left: gap, top, right: window.innerWidth - gap, bottom: window.innerHeight };
}

const sameBox = (a: Box | null, b: Box) =>
  !!a && a.left === b.left && a.top === b.top && a.right === b.right && a.bottom === b.bottom;

/** Where docked widget windows hang (ADR 0419): under the thread pane's title
 *  row, at its width. Null while that row is off screen, such as a phone
 *  swiped to another pane, so a docked window hides with its thread. */
export function useDockBox(active: boolean): Box | null {
  const [box, setBox] = useState<Box | null>(null);
  // A thread switch can draw a new title row, and a row that mounts late
  // bumps `found`.
  const focused = focusedThreadId.value;
  const [found, setFound] = useState(0);
  useLayoutEffect(() => {
    if (!active) return undefined;
    const row = titleRow();
    let onScreen = true;
    const measure = () => {
      if (!onScreen) {
        setBox(null);
        return;
      }
      const r = row?.getBoundingClientRect();
      const next = r ? { left: r.left, top: r.bottom, right: r.right, bottom: window.innerHeight } : underHeader();
      setBox((prev) => (sameBox(prev, next) ? prev : next));
    };
    measure();
    const resize = new ResizeObserver(measure);
    if (row) resize.observe(row);
    const visibility = row && typeof IntersectionObserver !== 'undefined'
      ? new IntersectionObserver(([entry]) => {
        onScreen = entry.isIntersecting;
        measure();
      })
      : null;
    if (row) visibility?.observe(row);
    window.addEventListener('resize', measure);
    // No row yet, such as on a cold open: hang under the header until one mounts.
    const arrival = row ? null : new MutationObserver(() => {
      if (titleRow()) setFound((n) => n + 1);
    });
    arrival?.observe(document.body, { childList: true, subtree: true });
    return () => {
      resize.disconnect();
      visibility?.disconnect();
      arrival?.disconnect();
      window.removeEventListener('resize', measure);
    };
  }, [active, focused, found]);
  return active ? box : null;
}
