import { useLayoutEffect, useState } from 'preact/hooks';
import { focusedThreadId } from '../../store/store';
import type { DockSpan } from '../../store/widgetWindows';
import { getRemPx } from '../../utils/dom';
import { viewportIsMobile } from '../../utils/viewport';

/** The gap docked windows keep from the screen's edges and from each other,
 *  in rem. */
export const DOCK_GAP_REM = 0.5;

/** Where docked windows hang: between two edges, from a top that is a CSS
 *  length, so a desktop dock follows the header's banners as they come and go. */
export interface DockLine extends DockSpan {
  top: string;
}

/** The title rows a phone's docked windows hang under. */
const TITLE_ROWS = '.thread-view-header, .mobile-thread-title-row';

/** The first match with a size. Only one layout's pane tree is mounted, so
 *  that is the mounted one, wherever a phone swipe has moved it. */
function shown(selector: string): HTMLElement | null {
  return [...document.querySelectorAll<HTMLElement>(selector)].find((el) => el.getBoundingClientRect().width > 0) ?? null;
}

/** Where docked windows hang. A desktop docks under the app header and its
 *  banners, across the window. A phone docks under the thread's title row, so
 *  it never covers the title, and nowhere while that row is off screen. A
 *  phone with no title row, such as the empty compose view, docks under the
 *  header. */
export function dockLineFor(view: {
  phone: boolean;
  viewportWidth: number;
  row: { left: number; right: number; bottom: number } | null;
  rowOnScreen: boolean;
  gap: number;
}): DockLine | null {
  const { viewportWidth, row, gap } = view;
  if (!view.phone || !row) return { left: gap, right: viewportWidth - gap, top: 'var(--app-header-bottom)' };
  if (!view.rowOnScreen) return null;
  return { left: row.left + gap, right: row.right - gap, top: `${row.bottom}px` };
}

const sameLine = (a: DockLine | null, b: DockLine | null) =>
  a === b || (!!a && !!b && a.left === b.left && a.right === b.right && a.top === b.top);

/** The live dock line (ADR 0419), null while nothing is docked. */
export function useDockLine(active: boolean): DockLine | null {
  const [line, setLine] = useState<DockLine | null>(null);
  const phone = viewportIsMobile.value;
  // A thread switch can draw a new title row, and a row that mounts late
  // bumps `found`.
  const focused = focusedThreadId.value;
  const [found, setFound] = useState(0);
  useLayoutEffect(() => {
    if (!active) return undefined;
    const row = phone ? shown(TITLE_ROWS) : null;
    let rowOnScreen = true;
    const measure = () => {
      const next = dockLineFor({
        phone,
        viewportWidth: window.innerWidth,
        row: row?.getBoundingClientRect() ?? null,
        rowOnScreen,
        gap: DOCK_GAP_REM * getRemPx(),
      });
      setLine((prev) => (sameLine(prev, next) ? prev : next));
    };
    measure();
    const resize = new ResizeObserver(measure);
    if (row) resize.observe(row);
    const visibility = row && typeof IntersectionObserver !== 'undefined'
      ? new IntersectionObserver(([entry]) => {
        rowOnScreen = entry.isIntersecting;
        measure();
      })
      : null;
    if (row) visibility?.observe(row);
    window.addEventListener('resize', measure);
    // A phone with no row yet, such as on a cold open, docks under the header
    // until one mounts.
    const arrival = phone && !row ? new MutationObserver(() => {
      if (shown(TITLE_ROWS)) setFound((n) => n + 1);
    }) : null;
    arrival?.observe(document.body, { childList: true, subtree: true });
    return () => {
      resize.disconnect();
      visibility?.disconnect();
      arrival?.disconnect();
      window.removeEventListener('resize', measure);
    };
  }, [active, phone, focused, found]);
  return active ? line : null;
}
